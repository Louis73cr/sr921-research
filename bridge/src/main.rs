// sr921-bridge: passive eBUS -> Home Assistant MQTT bridge for the Saunier Duval
// SR921 gateway. Eavesdrops the Vaillant stack's own D-Bus traffic (read-only for
// sensors), decodes eBUS telegrams, and publishes to MQTT with HA discovery.
// Also exposes two controls (heating mode, DHW setpoint) that write back through the
// Vaillant eBUS router (which arbitrates the bus) -- the heat curve is never touched.
// Zero dependencies: drives the on-device `busctl`, speaks just enough MQTT 3.1.1.
//
// Env: MQTT_HOST (127.0.0.1), MQTT_PORT (1883), MQTT_USER, MQTT_PASS, DEVICE_NAME,
//      SOURCE_CMD (default: busctl monitor of the eBUS router).

use std::collections::HashSet;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy)]
enum Ty { Exp, Ulg, Uin, Sch }

struct Def {
    zz: u8, sb: u8, req: &'static str,
    key: &'static str, name: &'static str, unit: &'static str,
    dclass: &'static str, sclass: &'static str,
    ty: Ty, ign: usize, factor: f64,
}

// Heat-pump (HMU 0x08/0x76) registers from the community ebusd config.
static DEFS: &[Def] = &[
    Def{zz:0x08,sb:0x09,req:"5402005b0d",key:"elec_power",name:"Conso electrique",unit:"W",dclass:"power",sclass:"measurement",ty:Ty::Exp,ign:4,factor:1.0},
    Def{zz:0x08,sb:0x09,req:"5402005e0d",key:"heat_output",name:"Puissance chaleur",unit:"W",dclass:"power",sclass:"measurement",ty:Ty::Exp,ign:4,factor:1.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3210",key:"yield_hc",name:"Energie chauffage cumulee",unit:"kWh",dclass:"energy",sclass:"total_increasing",ty:Ty::Ulg,ign:3,factor:1.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3200",key:"yield_hc_day",name:"Energie chauffage jour",unit:"kWh",dclass:"energy",sclass:"total_increasing",ty:Ty::Uin,ign:3,factor:10.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3211",key:"cop_hc",name:"COP chauffage",unit:"",dclass:"",sclass:"measurement",ty:Ty::Uin,ign:3,factor:10.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3216",key:"yield_hwc",name:"Energie ECS cumulee",unit:"kWh",dclass:"energy",sclass:"total_increasing",ty:Ty::Ulg,ign:3,factor:1.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3217",key:"cop_hwc",name:"COP ECS",unit:"",dclass:"",sclass:"measurement",ty:Ty::Uin,ign:3,factor:10.0},
    Def{zz:0x08,sb:0x1a,req:"05ff324d",key:"energy_total",name:"Energie electrique totale",unit:"kWh",dclass:"energy",sclass:"total_increasing",ty:Ty::Ulg,ign:3,factor:1.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3223",key:"therm_power",name:"Puissance thermique",unit:"kW",dclass:"power",sclass:"measurement",ty:Ty::Sch,ign:3,factor:10.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3224",key:"elec_power_kw",name:"Puissance electrique",unit:"kW",dclass:"power",sclass:"measurement",ty:Ty::Sch,ign:3,factor:10.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3400",key:"compressor_hours",name:"Heures compresseur",unit:"h",dclass:"duration",sclass:"total_increasing",ty:Ty::Uin,ign:3,factor:1.0},
    Def{zz:0x08,sb:0x1a,req:"05ff3401",key:"compressor_starts",name:"Demarrages compresseur",unit:"",dclass:"",sclass:"total_increasing",ty:Ty::Uin,ign:3,factor:1.0},
];

fn decode(ty: Ty, resp: &[u8], ign: usize, factor: f64) -> Option<f64> {
    let b = resp.get(ign..)?;
    let v = match ty {
        Ty::Exp => { if b.len()<4 {return None} let x=f32::from_le_bytes([b[0],b[1],b[2],b[3]]); if !x.is_finite() {return None} x as f64 }
        Ty::Ulg => { if b.len()<4 {return None} u32::from_le_bytes([b[0],b[1],b[2],b[3]]) as f64 }
        Ty::Uin => { if b.len()<2 {return None} let x=u16::from_le_bytes([b[0],b[1]]); if x==0xFFFF {return None} x as f64 }
        Ty::Sch => { if b.len()<2 {return None} let x=i16::from_le_bytes([b[0],b[1]]); if x==0x7FFF {return None} x as f64 }
    };
    Some(v / factor)
}

struct Hit { key: String, name: String, unit: String, dclass: String, sclass: String, value: f64 }

// Friendly names for 0x15 (sensoCOMFORT VRC720) b524 registers, keyed by the req[2..]
// index hex (ebusd 15.ctlv2). Unknown indexes fall back to a generic ctrl_<idx>.
fn ctrl_name(idx: &str) -> (String, &'static str, &'static str) {
    match idx {
        "00003900" => ("Pression d'eau".into(), "bar", "pressure"),
        "00007300" => ("Temperature exterieure".into(), "°C", "temperature"),
        "01000400" => ("Consigne ECS".into(), "°C", "temperature"),
        "01000500" => ("Temperature ballon ECS".into(), "°C", "temperature"),
        "02000800" => ("Temperature depart HC1".into(), "°C", "temperature"),
        "02000f00" => ("Courbe de chauffe HC1".into(), "", ""),
        "02001200" => ("Consigne depart min HC1".into(), "°C", "temperature"),
        "02001400" => ("Limite ete HC1".into(), "°C", "temperature"),
        "03010900" => ("Consigne nuit Zone 2".into(), "°C", "temperature"),
        "03010f00" => ("Temperature piece Zone 2".into(), "°C", "temperature"),
        "03011400" => ("Consigne piece active Zone 2".into(), "°C", "temperature"),
        "03012200" => ("Consigne jour Zone 2".into(), "°C", "temperature"),
        "03012800" => ("Humidite Zone 2".into(), "%", "humidity"),
        _ => (format!("ctrl_{}", idx), "°C", "temperature"),
    }
}

// Split a response telegram's bytes into (zz, sb, req, resp).
fn parts(b: &[u8]) -> Option<(u8, u8, &[u8], &[u8])> {
    if b.len() < 6 { return None; }
    let (zz, sb, rnn) = (b[1], b[3], b[4] as usize);
    if 5 + rnn >= b.len() { return None; }
    let req = &b[5..5+rnn];
    let resp_nn = b[5+rnn] as usize;
    let resp = b.get(6+rnn..6+rnn+resp_nn)?;
    Some((zz, sb, req, resp))
}

fn decode_telegram(b: &[u8]) -> Vec<Hit> {
    let mut out = Vec::new();
    let (zz, sb, req, resp) = match parts(b) { Some(x) => x, None => return out };
    let reqhex: String = req.iter().map(|x| format!("{:02x}", x)).collect();

    for d in DEFS {
        if d.zz == zz && d.sb == sb && d.req == reqhex {
            if let Some(v) = decode(d.ty, resp, d.ign, d.factor) {
                out.push(Hit{key:d.key.into(),name:d.name.into(),unit:d.unit.into(),dclass:d.dclass.into(),sclass:d.sclass.into(),value:v});
            }
        }
    }
    // Controller 0x15 b524 register reads -> float32 LE at the tail.
    if zz == 0x15 && sb == 0x24 && req.len() >= 6 && req[0] == 2 && req[1] == 0 && resp.len() >= 4 {
        let idx: String = req[2..].iter().map(|b| format!("{:02x}", b)).collect();
        if idx == "03010600" { return out; } // Z2OpMode -> exposed as the heat_mode select, not a sensor
        let t = &resp[resp.len()-4..];
        if t == [0xff,0xff,0xff,0x7f] || t == [0xff,0xff,0xff,0xff] { return out; } // invalid/absent
        let x = f32::from_le_bytes([t[0],t[1],t[2],t[3]]);
        if !x.is_finite() || x.abs() >= 1000.0 { return out; }
        let (name, unit, dclass) = ctrl_name(&idx);
        let named = !name.starts_with("ctrl_");
        // Hygiene: don't clutter HA with unused registers that sit at 0.
        if !named && x.abs() < 0.05 { return out; }
        out.push(Hit{key:format!("ctrl_{}", idx), name, unit:unit.into(), dclass:dclass.into(), sclass:"measurement".into(), value:(x as f64*100.0).round()/100.0});
    }
    out
}

// Zone 2 operating mode (Z2OpMode, read 02 00 03 01 06 00) -> text state for the select.
fn mode_of(b: &[u8]) -> Option<&'static str> {
    let (zz, sb, req, resp) = parts(b)?;
    if zz == 0x15 && sb == 0x24 && req == [2,0,3,1,6,0] && resp.len() >= 6 {
        let v = resp[4] as u16 | ((resp[5] as u16) << 8);
        return Some(match v { 0=>"off", 1=>"auto", 2=>"day", 3=>"night", _=>return None });
    }
    None
}

// ---- minimal MQTT 3.1.1 (CONNECT / PUBLISH / SUBSCRIBE) ----
struct Mqtt { host: String, port: u16, user: String, pass: String, client_id: String, s: Option<TcpStream> }

fn put_str(v: &mut Vec<u8>, s: &str) { v.extend_from_slice(&(s.len() as u16).to_be_bytes()); v.extend_from_slice(s.as_bytes()); }
fn put_len(v: &mut Vec<u8>, mut n: usize) { loop { let mut b=(n%128) as u8; n/=128; if n>0 {b|=0x80;} v.push(b); if n==0 {break;} } }

impl Mqtt {
    fn connect(&mut self) -> std::io::Result<()> {
        let mut s = TcpStream::connect((self.host.as_str(), self.port))?;
        s.set_nodelay(true).ok();
        s.set_write_timeout(Some(Duration::from_secs(10)))?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut vh = vec![0u8,4]; vh.extend_from_slice(b"MQTT"); vh.push(4);
        let mut flags = 0x02u8; // clean session, keepalive 0
        if !self.user.is_empty() { flags |= 0x80; if !self.pass.is_empty() { flags |= 0x40; } }
        vh.push(flags); vh.extend_from_slice(&[0,0]);
        put_str(&mut vh, &self.client_id);
        if !self.user.is_empty() { put_str(&mut vh, &self.user); if !self.pass.is_empty() { put_str(&mut vh, &self.pass); } }
        let mut pkt = vec![0x10u8]; put_len(&mut pkt, vh.len()); pkt.extend_from_slice(&vh);
        s.write_all(&pkt)?;
        let mut ack = [0u8;4]; s.read_exact(&mut ack)?;
        if ack[3] != 0 { return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("CONNACK rc={}", ack[3]))); }
        self.s = Some(s); Ok(())
    }
    fn publish(&mut self, topic: &str, payload: &str, retain: bool) -> std::io::Result<()> {
        for attempt in 0..2 {
            if self.s.is_none() { self.connect()?; }
            let mut vh = Vec::new(); put_str(&mut vh, topic); vh.extend_from_slice(payload.as_bytes());
            let mut pkt = vec![0x30u8 | if retain {1} else {0}]; put_len(&mut pkt, vh.len()); pkt.extend_from_slice(&vh);
            match self.s.as_mut().unwrap().write_all(&pkt) {
                Ok(()) => return Ok(()),
                Err(e) => { self.s = None; if attempt == 1 { return Err(e); } }
            }
        }
        Ok(())
    }
    fn subscribe(&mut self, topic: &str, pid: u16) -> std::io::Result<()> {
        let mut vh = Vec::new(); vh.extend_from_slice(&pid.to_be_bytes()); put_str(&mut vh, topic); vh.push(0);
        let mut pkt = vec![0x82u8]; put_len(&mut pkt, vh.len()); pkt.extend_from_slice(&vh);
        self.s.as_mut().unwrap().write_all(&pkt)
    }
    // Read one MQTT packet; return Some((topic,payload)) for a PUBLISH, else None.
    fn read_packet(&mut self) -> std::io::Result<Option<(String, String)>> {
        let s = self.s.as_mut().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotConnected, "no conn"))?;
        let mut h = [0u8;1]; s.read_exact(&mut h)?;
        let mut mult = 1usize; let mut len = 0usize;
        loop { let mut b=[0u8;1]; s.read_exact(&mut b)?; len += (b[0]&0x7f) as usize * mult; if b[0]&0x80==0 {break} mult*=128; if mult>2097152 {break} }
        let mut body = vec![0u8; len]; s.read_exact(&mut body)?;
        if h[0] & 0xF0 == 0x30 {
            if body.len() < 2 { return Ok(None); }
            let tl = ((body[0] as usize)<<8) | body[1] as usize;
            let qos = (h[0]>>1) & 3;
            let mut off = 2 + tl;
            if qos > 0 { off += 2; } // packet id
            if body.len() < off { return Ok(None); }
            let topic = String::from_utf8_lossy(&body[2..2+tl]).to_string();
            let payload = String::from_utf8_lossy(&body[off..]).to_string();
            return Ok(Some((topic, payload)));
        }
        Ok(None)
    }
}

fn device_json(device: &str) -> String {
    format!("\"dev\":{{\"ids\":[\"sr921\"],\"name\":\"{}\",\"mf\":\"Saunier Duval\",\"mdl\":\"GeniaSet Mono / SR921\"}}", device)
}

fn config_json(h: &Hit, device: &str) -> String {
    let mut j = format!("{{\"name\":\"{}\",\"stat_t\":\"sr921/{}\",\"uniq_id\":\"sr921_{}\",{}", h.name, h.key, h.key, device_json(device));
    if !h.unit.is_empty()   { j += &format!(",\"unit_of_meas\":\"{}\"", h.unit); }
    if !h.dclass.is_empty() { j += &format!(",\"dev_cla\":\"{}\"", h.dclass); }
    if !h.sclass.is_empty() { j += &format!(",\"stat_cla\":\"{}\"", h.sclass); }
    j.push('}'); j
}

fn fmt_val(v: f64) -> String { if v == v.trunc() { format!("{}", v as i64) } else { format!("{:.2}", v) } }

fn bytes_of(block: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in block.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("BYTE ") {
            if let Ok(n) = rest.trim_end_matches(';').trim().parse::<u16>() { out.push(n as u8); }
        }
    }
    out
}

// ---- write path: inject an eBUS telegram through the Vaillant router (it arbitrates
// the bus). Reads/writes always go via the router; we never become a 2nd bus master. ----
fn write_telegram(data: &[u8]) {
    let mut args: Vec<String> = ["call","de.vaillant.ebus_router","/","de.vaillant.havai_os.ebus_router.client_to_router","send_outbound_request","yyyyyayb","241","21","181","36"]
        .iter().map(|s| s.to_string()).collect();
    args.push(data.len().to_string());   // NN (5th byte of the telegram)
    args.push(data.len().to_string());   // array element count
    for b in data { args.push(b.to_string()); }
    args.push("false".into());
    match Command::new("busctl").args(&args).output() {
        Ok(o) => eprintln!("[write] data={:?} -> {}{}", data, String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => eprintln!("[write] busctl error: {}", e),
    }
}

fn handle_command(topic: &str, payload: &str) {
    let p = payload.trim();
    match topic {
        "sr921/set/heat_mode" => {
            let v: u8 = match p {
                "off" => 0, "auto" => 1, "day" => 2, "night" => 3,
                _ => match p.parse::<u8>() { Ok(n) if n <= 3 => n, _ => { eprintln!("[cmd] heat_mode rejete: {}", p); return; } },
            };
            eprintln!("[cmd] heat_mode -> {} ({})", p, v);
            write_telegram(&[2, 1, 3, 1, 6, 0, v, 0]); // Z2OpMode write, UIN LE
        }
        "sr921/set/dhw_setpoint" => {
            match p.parse::<f32>() {
                Ok(mut f) => {
                    if f < 30.0 { f = 30.0; } if f > 60.0 { f = 60.0; } // clamp
                    let b = f.to_le_bytes();
                    eprintln!("[cmd] dhw_setpoint -> {}", f);
                    write_telegram(&[2, 1, 1, 0, 4, 0, b[0], b[1], b[2], b[3]]); // HwcTempDesired write, EXP
                }
                Err(_) => eprintln!("[cmd] dhw_setpoint rejete: {}", p),
            }
        }
        _ => {}
    }
}

fn command_loop(host: String, port: u16, user: String, pass: String) {
    loop {
        let mut m = Mqtt { host: host.clone(), port, user: user.clone(), pass: pass.clone(), client_id: "sr921-bridge-cmd".into(), s: None };
        if m.connect().is_err() { thread::sleep(Duration::from_secs(5)); continue; }
        if let Some(s) = m.s.as_ref() { let _ = s.set_read_timeout(None); } // block for commands
        let _ = m.subscribe("sr921/set/heat_mode", 1);
        let _ = m.subscribe("sr921/set/dhw_setpoint", 2);
        eprintln!("[cmd] abonne aux commandes");
        loop {
            match m.read_packet() {
                Ok(Some((t, p))) => handle_command(&t, &p),
                Ok(None) => {}
                Err(_) => break,
            }
        }
        thread::sleep(Duration::from_secs(3));
    }
}

fn main() {
    let host = env::var("MQTT_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = env::var("MQTT_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(1883);
    let user = env::var("MQTT_USER").unwrap_or_default();
    let pass = env::var("MQTT_PASS").unwrap_or_default();
    let device = env::var("DEVICE_NAME").unwrap_or_else(|_| "Saunier Duval Heat Pump".into());
    let source = env::var("SOURCE_CMD").unwrap_or_else(|_| "busctl monitor de.vaillant.ebus_router de.vaillant.ebusclient_agent".into());

    // command listener (own MQTT connection / client id)
    { let (h,u,p)=(host.clone(),user.clone(),pass.clone()); thread::spawn(move || command_loop(h, port, u, p)); }

    let mut mqtt = Mqtt { host: host.clone(), port, user, pass, client_id: "sr921-bridge".into(), s: None };
    let publish = match mqtt.connect() {
        Ok(()) => { eprintln!("[bridge] MQTT connecte a {}:{}", host, port); true }
        Err(e) => { eprintln!("[bridge] MQTT indisponible ({}), mode log-seul", e); false }
    };
    if publish {
        // control entities (discovery), published once at startup
        let sel = format!("{{\"name\":\"Mode chauffage\",\"stat_t\":\"sr921/heat_mode\",\"cmd_t\":\"sr921/set/heat_mode\",\"options\":[\"off\",\"auto\",\"day\",\"night\"],\"uniq_id\":\"sr921_heat_mode\",{}}}", device_json(&device));
        let _ = mqtt.publish("homeassistant/select/sr921/heat_mode/config", &sel, true);
        let num = format!("{{\"name\":\"Consigne ECS\",\"stat_t\":\"sr921/ctrl_01000400\",\"cmd_t\":\"sr921/set/dhw_setpoint\",\"min\":30,\"max\":60,\"step\":1,\"unit_of_meas\":\"°C\",\"dev_cla\":\"temperature\",\"uniq_id\":\"sr921_dhw_setpoint\",{}}}", device_json(&device));
        let _ = mqtt.publish("homeassistant/number/sr921/dhw_setpoint/config", &num, true);
    }

    let mut child = Command::new("sh").arg("-c").arg(&source).stdout(Stdio::piped()).spawn().expect("spawn source");
    let rdr = BufReader::new(child.stdout.take().unwrap());
    let mut seen: HashSet<String> = HashSet::new();
    let mut block = String::new();

    let handle = |block: &str, mqtt: &mut Mqtt, seen: &mut HashSet<String>| {
        if !block.contains("send_outbound_response") { return; }
        let bytes = bytes_of(block);
        for h in decode_telegram(&bytes) {
            eprintln!("[val] {:<26} = {} {}", h.key, fmt_val(h.value), h.unit);
            if !publish { continue; }
            if seen.insert(h.key.clone()) {
                let _ = mqtt.publish(&format!("homeassistant/sensor/sr921/{}/config", h.key), &config_json(&h, &device), true);
            }
            let _ = mqtt.publish(&format!("sr921/{}", h.key), &fmt_val(h.value), true);
        }
        if let Some(mode) = mode_of(&bytes) {
            eprintln!("[val] heat_mode                 = {}", mode);
            if publish { let _ = mqtt.publish("sr921/heat_mode", mode, true); }
        }
    };

    for line in rdr.lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.starts_with("> ") && !block.is_empty() { handle(&block, &mut mqtt, &mut seen); block.clear(); }
        block.push_str(&line); block.push('\n');
    }
    handle(&block, &mut mqtt, &mut seen);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_and_modes() {
        // 0x08 b509 5402005b0d -> electric power 10.4 W
        let tg = [0xf1,0x08,0xb5,0x09,0x05,0x54,0x02,0x00,0x5b,0x0d, 0x08,0x02,0x01,0x5b,0x0d,0x66,0x66,0x26,0x41];
        let hits = decode_telegram(&tg);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "elec_power");
        assert!((hits[0].value - 10.4).abs() < 0.01);
        // controller 0x15 b524 020003012200 -> 20.0 named "Consigne jour Zone 2"
        let c = [0xf1,0x15,0xb5,0x24,0x06,0x02,0x00,0x03,0x01,0x22,0x00, 0x08,0x02,0x03,0x22,0x00,0x00,0x00,0xa0,0x41];
        let hc = decode_telegram(&c);
        assert_eq!(hc.len(), 1);
        assert_eq!(hc[0].key, "ctrl_03012200");
        assert_eq!(hc[0].name, "Consigne jour Zone 2");
        // unnamed register sitting at 0 is suppressed (hygiene)
        let z = [0xf1,0x15,0xb5,0x24,0x06,0x02,0x00,0x02,0x00,0x1a,0x00, 0x08,0x02,0x02,0x1a,0x00,0x00,0x00,0x00,0x00];
        assert_eq!(decode_telegram(&z).len(), 0);
        // Z2OpMode read 020003010600 -> "off", and not a sensor
        let m = [0xf1,0x15,0xb5,0x24,0x06,0x02,0x00,0x03,0x01,0x06,0x00, 0x06,0x03,0x03,0x06,0x00,0x00,0x00];
        assert_eq!(decode_telegram(&m).len(), 0);
        assert_eq!(mode_of(&m), Some("off"));
    }
}
