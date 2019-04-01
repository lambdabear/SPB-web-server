use actix_web::fs::NamedFile;
use actix_web::*;
use futures::future::Future;
use ip_addr_op::*;
use ipnetwork::*;
use serde::{Deserialize, Serialize};

use std::net::{IpAddr, Ipv4Addr};
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
    gateway_ip: &'static str,
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
}

impl Clone for State {
    fn clone(&self) -> Self {
        State {
            ifname: self.ifname.clone(),
            handle: self.handle.clone(),
        }
    }
}

fn get_status(req: &HttpRequest<State>) -> impl Responder {
    let (ip_addr, mask) = match get_ip_addrs(req.state().handle.clone(), req.state().ifname.clone())
    {
        Ok(addrs) => {
            let (addr, prefix) = addrs[1];
            let mask = Ipv4Network::new(addr, prefix)
                .expect("parse mask error")
                .mask();
            (addr.to_string(), mask.to_string())
        }
        Err(_) => (String::from(""), String::from("")),
    };
    Status {
        host_name: "***分行人民街分理处",
        host_ip: ip_addr,
        host_mask: mask,
        gateway_ip: "10.0.8.1",
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
    req.json()
        .from_err()
        .and_then(|val: HostSetting| {
            let mask = &val.mask;
            let mask: Ipv4Addr = mask.parse().expect("parse mask error");
            let prefix: u8 = ipv4_mask_to_prefix(mask).expect("mask to prefix error");
            let new_ip: Ipv4Addr = (&val.ip).parse().expect("parse ip address error");

            match set_ip_addr(handle, ifname, Ipv4Addr::new(10, 0, 0, 9), new_ip, prefix) {
                Ok(_) => Ok(HttpResponse::Ok().json(val)), // <- send response
                Err(_) => Ok(HttpResponse::BadRequest().json(val)),
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
        ifname: String::from("enp7s0"),
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
