// sr921-bridge: passive eBUS -> Home Assistant MQTT bridge for the Saunier Duval
// SR921 gateway. Eavesdrops the Vaillant stack's own D-Bus traffic (read-only, the
// bus is never written), decodes the eBUS telegrams, and publishes to MQTT with
// Home Assistant discovery. Zero dependencies: it drives the on-device `busctl`
// and speaks just enough MQTT 3.1.1 to CONNECT + PUBLISH.
//
// Config via env: MQTT_HOST (default 127.0.0.1), MQTT_PORT (1883), MQTT_USER,
// MQTT_PASS, DEVICE_NAME, SOURCE_CMD (default: busctl monitor of the eBUS router).

use std::collections::HashSet;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Ty { Exp, Ulg, Uin, Sch }

struct Def {
    zz: u8, sb: u8, req: &'static str,
    key: &'static str, name: &'static str, unit: &'static str,
    dclass: &'static str, sclass: &'static str,
    ty: Ty, ign: usize, factor: f64,
}

// Known Vaillant heat-pump (HMU, addr 0x08/0x76) registers. Names/types from the
// community ebusd config. b509 = run data, b51a = energy/COP counters (polled slowly).
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

// Given a full response telegram's bytes [QQ ZZ PB SB reqNN req.. respNN resp..],
// return every sensor reading it carries.
fn decode_telegram(b: &[u8]) -> Vec<Hit> {
    let mut out = Vec::new();
    if b.len() < 6 { return out; }
    let (zz, sb, rnn) = (b[1], b[3], b[4] as usize);
    if 5 + rnn >= b.len() { return out; }
    let req = &b[5..5+rnn];
    let resp_nn = b[5+rnn] as usize;
    let resp = b.get(6+rnn..6+rnn+resp_nn).unwrap_or(&[]);
    let reqhex: String = req.iter().map(|x| format!("{:02x}", x)).collect();

    for d in DEFS {
        if d.zz == zz && d.sb == sb && d.req == reqhex {
            if let Some(v) = decode(d.ty, resp, d.ign, d.factor) {
                out.push(Hit{key:d.key.into(),name:d.name.into(),unit:d.unit.into(),dclass:d.dclass.into(),sclass:d.sclass.into(),value:v});
            }
        }
    }
    // Controller 0x15 b524 register reads: response ends in a float32 LE (setpoints
    // / temperatures). No official names yet -> generic ctrl_<idx> temperature sensors.
    if zz == 0x15 && sb == 0x24 && req.len() >= 4 && req[0] == 2 && req[1] == 0 && resp.len() >= 4 {
        let t = &resp[resp.len()-4..];
        let x = f32::from_le_bytes([t[0],t[1],t[2],t[3]]);
        if x.is_finite() && x.abs() < 300.0 {
            let idx: String = req[2..].iter().map(|b| format!("{:02x}", b)).collect();
            out.push(Hit{key:format!("ctrl_{}", idx),name:format!("Regulateur {}", idx),unit:"°C".into(),dclass:"temperature".into(),sclass:"measurement".into(),value:(x as f64*100.0).round()/100.0});
        }
    }
    out
}

// ---- minimal MQTT 3.1.1 (CONNECT + PUBLISH only) ----
struct Mqtt { host: String, port: u16, user: String, pass: String, s: Option<TcpStream> }

fn put_str(v: &mut Vec<u8>, s: &str) { v.extend_from_slice(&(s.len() as u16).to_be_bytes()); v.extend_from_slice(s.as_bytes()); }
fn put_len(v: &mut Vec<u8>, mut n: usize) { loop { let mut b=(n%128) as u8; n/=128; if n>0 {b|=0x80;} v.push(b); if n==0 {break;} } }

impl Mqtt {
    fn connect(&mut self) -> std::io::Result<()> {
        let mut s = TcpStream::connect((self.host.as_str(), self.port))?;
        s.set_write_timeout(Some(Duration::from_secs(10)))?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut vh = vec![0u8,4]; vh.extend_from_slice(b"MQTT"); vh.push(4);
        let mut flags = 0x02u8; // clean session, keepalive 0
        if !self.user.is_empty() { flags |= 0x80; if !self.pass.is_empty() { flags |= 0x40; } }
        vh.push(flags); vh.extend_from_slice(&[0,0]);
        put_str(&mut vh, "sr921-bridge");
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
}

fn config_json(h: &Hit, device: &str) -> String {
    let mut j = format!("{{\"name\":\"{}\",\"stat_t\":\"sr921/{}\",\"uniq_id\":\"sr921_{}\",\"dev\":{{\"ids\":[\"sr921\"],\"name\":\"{}\",\"mf\":\"Saunier Duval\",\"mdl\":\"GeniaSet Mono / SR921\"}}", h.name, h.key, h.key, device);
    if !h.unit.is_empty()   { j += &format!(",\"unit_of_meas\":\"{}\"", h.unit); }
    if !h.dclass.is_empty() { j += &format!(",\"dev_cla\":\"{}\"", h.dclass); }
    if !h.sclass.is_empty() { j += &format!(",\"stat_cla\":\"{}\"", h.sclass); }
    j.push('}'); j
}

fn fmt_val(v: f64) -> String { if v == v.trunc() { format!("{}", v as i64) } else { format!("{:.2}", v) } }

// Pull the ordered `BYTE n` values out of one busctl-monitor message block.
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

fn main() {
    let host = env::var("MQTT_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = env::var("MQTT_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(1883);
    let user = env::var("MQTT_USER").unwrap_or_default();
    let pass = env::var("MQTT_PASS").unwrap_or_default();
    let device = env::var("DEVICE_NAME").unwrap_or_else(|_| "Saunier Duval Heat Pump".into());
    let source = env::var("SOURCE_CMD").unwrap_or_else(|_| "busctl monitor de.vaillant.ebus_router de.vaillant.ebusclient_agent".into());

    let mut mqtt = Mqtt { host: host.clone(), port, user, pass, s: None };
    let publish = match mqtt.connect() {
        Ok(()) => { eprintln!("[bridge] MQTT connecte a {}:{}", host, port); true }
        Err(e) => { eprintln!("[bridge] MQTT indisponible ({}), mode log-seul", e); false }
    };

    let mut child = Command::new("sh").arg("-c").arg(&source)
        .stdout(Stdio::piped()).spawn().expect("spawn source");
    let rdr = BufReader::new(child.stdout.take().unwrap());
    let mut seen: HashSet<String> = HashSet::new();
    let mut block = String::new();

    let handle = |block: &str, mqtt: &mut Mqtt, seen: &mut HashSet<String>| {
        if !block.contains("send_outbound_response") { return; }
        for h in decode_telegram(&bytes_of(block)) {
            eprintln!("[val] {:<26} = {} {}", h.key, fmt_val(h.value), h.unit);
            if !publish { continue; }
            if seen.insert(h.key.clone()) {
                let _ = mqtt.publish(&format!("homeassistant/sensor/sr921/{}/config", h.key), &config_json(&h, &device), true);
            }
            let _ = mqtt.publish(&format!("sr921/{}", h.key), &fmt_val(h.value), true);
        }
    };

    for line in rdr.lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.starts_with("> ") && !block.is_empty() {
            handle(&block, &mut mqtt, &mut seen);
            block.clear();
        }
        block.push_str(&line); block.push('\n');
    }
    handle(&block, &mut mqtt, &mut seen);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_real_capture() {
        // 0x08 b509 5402005b0d -> resp 02 01 5B 0D 66 66 26 41  (electric power 10.4 W)
        let tg = [0xf1,0x08,0xb5,0x09,0x05,0x54,0x02,0x00,0x5b,0x0d, 0x08,0x02,0x01,0x5b,0x0d,0x66,0x66,0x26,0x41];
        let hits = decode_telegram(&tg);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "elec_power");
        assert!((hits[0].value - 10.4).abs() < 0.01, "got {}", hits[0].value);
        // controller 0x15 b524 020003012200 -> ...00 00 A0 41 = 20.0
        let c = [0xf1,0x15,0xb5,0x24,0x06,0x02,0x00,0x03,0x01,0x22,0x00, 0x08,0x02,0x03,0x22,0x00,0x00,0x00,0xa0,0x41];
        let hc = decode_telegram(&c);
        assert_eq!(hc.len(), 1);
        assert_eq!(hc[0].key, "ctrl_03012200");
        assert!((hc[0].value - 20.0).abs() < 0.01, "got {}", hc[0].value);
    }
}
