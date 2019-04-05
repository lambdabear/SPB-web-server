use actix_web::fs::NamedFile;
use actix_web::*;
use crossbeam_channel::{Receiver, Sender};
use futures::future::Future;
use ip_addr_op::*;
use ipnetwork::*;
use iproute::*;
use serde::{Deserialize, Serialize};

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn index(_req: &HttpRequest<State>) -> Result<NamedFile> {
    let path = Path::new("./index.html");
    Ok(NamedFile::open(path)?)
}

#[derive(Serialize)]
struct Status {
    host_name: &'static str,
    host_ip: String,
    host_mask: String,
    gateway_ip: String,
    broker: String,
    power_status: String,
}

impl Responder for Status {
    type Item = HttpResponse;
    type Error = Error;

    fn respond_to<S>(self, _req: &HttpRequest<S>) -> Result<HttpResponse, Error> {
        let body = serde_json::to_string(&self)?;

        Ok(HttpResponse::Ok()
            .content_type("application/json")
            .body(body))
    }
}

#[derive(Clone)]
struct State {
    handle: Handle,
    ifname: String,
    config_ip: Ipv4Addr,
    status_buffer: Arc<Mutex<VecDeque<String>>>,
    broker: Arc<Mutex<Broker>>,
    broker_set_s: Sender<Broker>,
}

fn get_status(req: &HttpRequest<State>) -> impl Responder {
    let (ip_addr, mask) = match get_ip_addrs(req.state().handle.clone(), req.state().ifname.clone())
    {
        Ok(addrs) => addrs
            .iter()
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
    let broker = match req.state().broker.lock() {
        Ok(b) => match ((*b).url.clone(), (*b).port) {
            (s, 0) => s,
            (s, p) => format!("{} : {}", s, p),
        },
        Err(_) => "".to_string(),
    };
    let gateway = match get_default_routes() {
        Ok(ref routes) if routes.len() > 0 => routes[0].gateway().to_string(),
        _ => String::from(""),
    };
    let power_status = match req.state().status_buffer.lock() {
        Ok(buffer) => buffer.iter().fold("".to_string(), |acc, s| {
            format!("{} {}", acc, format!("[{}]", s))
        }),
        Err(_) => "".to_string(),
    };
    Status {
        host_name: "***分行人民街分理处",
        host_ip: ip_addr,
        host_mask: mask,
        gateway_ip: gateway.to_string(),
        broker,
        power_status,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HostSetting {
    name: String,
    ip: String,
    mask: String,
    gateway: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Broker {
    pub url: String,
    pub port: u16,
}

impl Broker {
    pub fn new(url: String, port: u16) -> Broker {
        Broker { url, port }
    }
}

fn set_host(req: &HttpRequest<State>) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let handle = req.state().handle.clone();
    let ifname = req.state().ifname.clone();
    let config_ip = req.state().config_ip.clone();
    req.json()
        .from_err()
        .and_then(move |val: HostSetting| {
            let mask = &val.mask;
            match mask.parse() {
                Ok(mask) => match (
                    ipv4_mask_to_prefix(mask),
                    (&val.ip).parse(),
                    (&val.gateway).parse(),
                ) {
                    (Ok(prefix), Ok(new_ip), Ok(gateway)) => {
                        // check if gateway is in this ip network
                        match Ipv4Network::new(new_ip, prefix) {
                            Ok(network) => {
                                if !network.contains(gateway) {
                                    eprintln!("gateway is not in this ip network");
                                    return Ok(HttpResponse::BadRequest().json(val));
                                }
                            }
                            Err(e) => {
                                eprintln!("{}", e);
                                return Ok(HttpResponse::BadRequest().json(val));
                            }
                        };
                        // set host ip address and mask
                        match set_ip_addr(handle, ifname.clone(), config_ip, new_ip, prefix) {
                            Ok(_) => {
                                // update new gateway
                                match update_default_route(ifname, gateway) {
                                    Ok(_) => Ok(HttpResponse::Ok().json(val)),
                                    Err(e) => {
                                        eprintln!("{}", e);
                                        Ok(HttpResponse::BadRequest().json(val))
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("{}", e);
                                Ok(HttpResponse::BadRequest().json(val))
                            }
                        }
                    }
                    _ => {
                        eprintln!("set host ip address and gateway failed");
                        Ok(HttpResponse::BadRequest().json(val))
                    }
                },
                Err(e) => {
                    eprintln!("{}", e);
                    Ok(HttpResponse::BadRequest().json(val))
                }
            }
        })
        .responder()
}

#[derive(Debug, Serialize, Deserialize)]
struct MqttServerSetting {
    ip: String,
    port: u16,
}

fn set_mqtt_server(req: &HttpRequest<State>) -> Box<Future<Item = HttpResponse, Error = Error>> {
    let broker_set_s = req.state().broker_set_s.clone();
    req.json()
        .from_err()
        .and_then(move |val: MqttServerSetting| {
            match broker_set_s.send(Broker::new(val.ip, val.port)) {
                Ok(()) => Ok(HttpResponse::Ok().json("")),
                Err(e) => {
                    eprintln!("{}", e);
                    Ok(HttpResponse::BadRequest().json(""))
                }
            }
        })
        .responder()
}

pub fn run(r: Receiver<Vec<u8>>, broker_r: Receiver<Broker>, broker_set_s: Sender<Broker>) {
    let handle = make_handle();
    let status_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(5)));
    let status_b = status_buffer.clone();
    let broker = Arc::new(Mutex::new(Broker::new("".to_string(), 0)));
    let broker1 = broker.clone();
    let state = State {
        handle,
        ifname: String::from("eth0"),
        config_ip: Ipv4Addr::new(10, 188, 188, 188),
        status_buffer,
        broker,
        broker_set_s,
    };

    // save receive message to status buffer every 100ms
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        match r.recv() {
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
            Err(e) => eprintln!("{}", e),
        }
    });

    // get updated broker message every 1 second
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

    server::new(move || {
        vec![
            App::with_state(state.clone())
                .prefix("/api")
                .resource("/status", |r| r.f(get_status))
                .resource("/setHost", |r| r.f(set_host))
                .resource("/setMqttServer", |r| r.f(set_mqtt_server)),
            App::with_state(state.clone()).resource("/", |r| r.f(index)),
        ]
    })
    .bind("0.0.0.0:8080")
    .unwrap()
    .run()
}
