use actix_web::fs::NamedFile;
use actix_web::*;
use futures::future::Future;
use ip_addr_op::*;
use ipnetwork::*;
use iproute::*;
use serde::{Deserialize, Serialize};

use std::net::Ipv4Addr;
use std::path::Path;

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
    server_ip: &'static str,
    connect_status: &'static str,
    power_status: &'static str,
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

#[derive(Debug)]
struct State {
    handle: Handle,
    ifname: String,
    config_ip: Ipv4Addr,
}

impl Clone for State {
    fn clone(&self) -> Self {
        State {
            ifname: self.ifname.clone(),
            handle: self.handle.clone(),
            config_ip: self.config_ip.clone(),
        }
    }
}

fn get_status(req: &HttpRequest<State>) -> impl Responder {
    let (ip_addr, mask) = match get_ip_addrs(req.state().handle.clone(), req.state().ifname.clone())
    {
        Ok(addrs) => {
            let (addr, prefix) = addrs[addrs.len() - 1];
            match Ipv4Network::new(addr, prefix) {
                Ok(a) => {
                    let mask = a.mask();
                    (addr.to_string(), mask.to_string())
                }
                Err(e) => {
                    eprintln!("{}", e);
                    (String::from(""), String::from(""))
                }
            }
        }
        Err(_) => (String::from(""), String::from("")),
    };
    let gateway = match get_default_routes() {
        Ok(ref routes) if routes.len() > 0 => routes[0].gateway().to_string(),
        _ => String::from(""),
    };
    Status {
        host_name: "***分行人民街分理处",
        host_ip: ip_addr,
        host_mask: mask,
        gateway_ip: gateway.to_string(),
        server_ip: "10.0.0.8:8080",
        connect_status: "已连接",
        power_status: "正常",
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HostSetting {
    name: String,
    ip: String,
    mask: String,
    gateway: String,
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
    req.json()
        .from_err()
        .and_then(|val: MqttServerSetting| {
            println!("model: {:?}", val); // TODO:
            Ok(HttpResponse::Ok().json(val)) // <- send response
        })
        .responder()
}

pub fn run() {
    let handle = make_handle();
    let state = State {
        handle,
        ifname: String::from("eth0"),
        config_ip: Ipv4Addr::new(10, 0, 0, 14),
    };
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
    .bind("0.0.0.0:80")
    .unwrap()
    .run()
}
