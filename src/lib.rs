use actix_files::NamedFile;
use actix_web::*;
use crossbeam_channel::{bounded, Receiver, Sender};
use ip_addr_op::*;
use ipnetwork::*;
use iproute::*;
use publicsuffix::Domain;
use serde::{Deserialize, Serialize};

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use spb_config::*;

fn index(_req: HttpRequest) -> Result<NamedFile> {
    let path = Path::new("./index.html");
    Ok(NamedFile::open(path)?)
}

#[derive(Serialize)]
struct Status {
    host_name: String,
    host_ip: String,
    host_mask: String,
    gateway_ip: String,
    broker: String,
    power_status: String,
}

#[derive(Clone, Debug)]
pub struct NetSetting {
    pub ip_addr: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Ipv4Addr,
}

impl NetSetting {
    fn new(ip_addr: Ipv4Addr, prefix: u8, gateway: Ipv4Addr) -> Self {
        NetSetting {
            ip_addr,
            prefix,
            gateway,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SaveMsg {
    DeviceName(String),
    Net(NetSetting),
    Broker(Broker),
}

#[derive(Clone)]
struct State {
    config_path: String,
    device_name: Arc<Mutex<String>>,
    handle: Handle,
    ifname: String,
    config_ip: Ipv4Addr,
    status_buffer: Arc<Mutex<VecDeque<String>>>,
    broker: Arc<Mutex<Broker>>,
    broker_set_s: Sender<Broker>,
    save_s: Sender<SaveMsg>,
    change_ip_notify_s: Sender<SaveMsg>,
    device_name_s: Sender<String>,
}

fn get_status(state: web::Data<State>) -> Result<HttpResponse> {
    let (ip_addr, mask) = match get_ip_addrs(state.handle.clone(), state.ifname.clone()) {
        Ok(addrs) => addrs
            .iter()
            .filter(|(addr, _)| *addr != state.config_ip)
            .map(|(addr, prefix)| match Ipv4Network::new(*addr, *prefix) {
                Ok(a) => (addr.to_string(), a.mask().to_string()),
                Err(_) => ("".to_string(), "".to_string()),
            })
            .fold(
                ("".to_string(), "".to_string()),
                |(addr_s, mask_s), (addr, mask)| {
                    (
                        format!("{}  {}", addr_s, addr),
                        format!("{}  {}", mask_s, mask),
                    )
                },
            ),
        Err(_) => (String::from(""), String::from("")),
    };
    let broker = match state.broker.lock() {
        Ok(b) => match ((*b).host.clone(), (*b).port) {
            (s, 0) => s,
            (s, p) => format!("{} : {}", s, p),
        },
        Err(_) => "".to_string(),
    };
    let gateway = match get_default_routes() {
        Ok(ref routes) if routes.len() > 0 => routes[0].gateway().to_string(),
        _ => String::from(""),
    };
    let power_status = match state.status_buffer.lock() {
        Ok(buffer) => buffer.iter().fold("".to_string(), |acc, s| {
            format!("{} {}", acc, format!("[{}]", s))
        }),
        Err(_) => "".to_string(),
    };
    let host_name = match state.device_name.lock() {
        Ok(name) => name.clone(),
        Err(e) => {
            eprintln!("{}", e);
            "".to_string()
        }
    };
    Ok(HttpResponse::Ok().json(Status {
        host_name,
        host_ip: ip_addr,
        host_mask: mask,
        gateway_ip: gateway.to_string(),
        broker,
        power_status,
    }))
}

#[derive(Debug, Serialize, Deserialize)]
struct NetworkSetting {
    ip: String,
    mask: String,
    gateway: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct NameSetting {
    name: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Broker {
    pub host: String,
    pub port: u16,
}

impl Broker {
    pub fn new(host: String, port: u16) -> Result<Broker, String> {
        if Domain::has_valid_syntax(&host) || (&host).parse::<Ipv4Addr>().is_ok() {
            Ok(Broker { host, port })
        } else {
            println!("broker domain invalid");
            Err("host parse error".to_string())
        }
    }
}

fn set_net(
    network_setting: web::Json<NetworkSetting>,
    state: web::Data<State>,
) -> Result<HttpResponse> {
    let handle = state.handle.clone();
    let ifname = state.ifname.clone();
    let config_ip = state.config_ip.clone();
    let save_s = state.save_s.clone();
    let change_ip_notify_s = state.change_ip_notify_s.clone();

    match network_setting.mask.parse() {
        Ok(mask) => match (
            ipv4_mask_to_prefix(mask),
            (network_setting.ip).parse(),
            (network_setting.gateway).parse(),
        ) {
            (Ok(prefix), Ok(new_ip), Ok(gateway)) => {
                // check if gateway is in this ip network
                match Ipv4Network::new(new_ip, prefix) {
                    Ok(network) => {
                        if !network.contains(gateway) {
                            eprintln!("gateway is not in this ip network");
                            return Ok(
                                HttpResponse::BadRequest().json(network_setting.into_inner())
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        return Ok(HttpResponse::BadRequest().json(network_setting.into_inner()));
                    }
                };

                // send changing ip address signal to the mqtt client thread
                match change_ip_notify_s
                    .send(SaveMsg::Net(NetSetting::new(new_ip, prefix, gateway)))
                {
                    Ok(()) => thread::sleep(Duration::from_millis(1000)),
                    Err(e) => eprintln!("{}", e),
                }

                // set host ip address and mask
                match set_ip_addr(handle, ifname.clone(), config_ip, new_ip, prefix) {
                    Ok(_) => {
                        // update new gateway
                        match update_default_route(ifname, gateway) {
                            Ok(_) => {
                                // send saving to config file message
                                match save_s
                                    .send(SaveMsg::Net(NetSetting::new(new_ip, prefix, gateway)))
                                {
                                    Ok(()) => (),
                                    Err(e) => eprintln!("{}", e),
                                };
                                Ok(HttpResponse::Ok().json(network_setting.into_inner()))
                            }
                            Err(e) => {
                                eprintln!("{}", e);
                                Ok(HttpResponse::BadRequest().json(network_setting.into_inner()))
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        Ok(HttpResponse::BadRequest().json(network_setting.into_inner()))
                    }
                }
            }
            _ => {
                eprintln!("set host ip address and gateway failed");
                Ok(HttpResponse::BadRequest().json(network_setting.into_inner()))
            }
        },
        Err(e) => {
            eprintln!("{}", e);
            Ok(HttpResponse::BadRequest().json(network_setting.into_inner()))
        }
    }
}

fn set_mqtt_server(broker: web::Json<Broker>, state: web::Data<State>) -> Result<HttpResponse> {
    let broker_set_s = state.broker_set_s.clone();
    match broker_set_s.send(broker.into_inner()) {
        Ok(()) => Ok(HttpResponse::Ok().json("")),
        Err(e) => {
            eprintln!("{:?}", e);
            Ok(HttpResponse::BadRequest().json("message sending error"))
        }
    }
}

fn set_name(name_setting: web::Json<NameSetting>, state: web::Data<State>) -> Result<HttpResponse> {
    let save_s = state.save_s.clone();
    let device_name_s = state.device_name_s.clone();

    // request to save new device name to config file
    match save_s.send(SaveMsg::DeviceName(name_setting.name.clone())) {
        Ok(()) => {
            // request to change state's device name
            match device_name_s.send(name_setting.name.clone()) {
                Ok(()) => (),
                Err(e) => eprintln!("{}", e),
            }
            Ok(HttpResponse::Ok().json(""))
        }
        Err(e) => {
            eprintln!("{}", e);
            Ok(HttpResponse::BadRequest().json("message sending error"))
        }
    }
}

pub fn run(
    config_path: String,
    config_ip: Ipv4Addr,
    ifname: String,
    r: Receiver<Vec<u8>>,
    broker_r: Receiver<Broker>,
    broker_set_s: Sender<Broker>,
    save_s: Sender<SaveMsg>,
    change_ip_notify_s: Sender<SaveMsg>,
) {
    // let config_path = "./config.json".to_string();
    let config = match read_config(&config_path) {
        Ok(conf) => conf,
        Err(e) => {
            eprintln!("{}", e);
            Config::init_config_val()
        }
    };
    let device_name = Arc::new(Mutex::new(config.device_name));
    let handle = make_handle().unwrap(); //TODO: can be panic
    let status_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(5)));
    let status_b = status_buffer.clone();
    let init_broker =
        Broker::new(config.broker_host, config.broker_port).expect("init_broker invalid");
    let broker = Arc::new(Mutex::new(init_broker));
    let broker1 = broker.clone();
    let (device_name_s, device_name_r) = bounded(1);
    let state = State {
        config_path,
        device_name: device_name.clone(),
        handle,
        ifname,
        config_ip,
        status_buffer,
        broker,
        broker_set_s,
        save_s,
        change_ip_notify_s,
        device_name_s,
    };

    // save receive message to status buffer and save new device name
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        match r.try_recv() {
            Ok(msg) => match String::from_utf8(msg) {
                Ok(s) => match status_b.lock() {
                    Ok(mut status) => {
                        if (*status).len() < 5 {
                            (*status).push_back(s)
                        } else {
                            (*status).pop_front();
                            (*status).push_back(s);
                        }
                    }
                    Err(e) => eprintln!("{}", e),
                },
                Err(e) => eprintln!("{}", e),
            },
            Err(_) => (),
        }
        match device_name_r.try_recv() {
            Ok(name) => match device_name.lock() {
                Ok(mut dev_name) => *dev_name = name,
                Err(e) => eprintln!("{}", e),
            },
            Err(_) => (),
        }
    });

    // get updated broker info message every 1 second
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        match broker_r.recv() {
            Ok(broker) => match broker1.lock() {
                Ok(mut b) => *b = broker,
                Err(e) => eprintln!("{}", e),
            },
            Err(e) => eprintln!("{}", e),
        }
    });

    HttpServer::new(move || {
        App::new()
            .register_data(web::Data::new(state.clone()))
            .service(
                web::scope("/api")
                    .route("/status", web::get().to(get_status))
                    .route("/setNet", web::post().to(set_net))
                    .route("/setMqttServer", web::post().to(set_mqtt_server))
                    .route("/setName", web::post().to(set_name)),
            )
            .route("/", web::get().to(index))
    })
    .bind("0.0.0.0:8080")
    .unwrap()
    .run()
    .unwrap()
}
