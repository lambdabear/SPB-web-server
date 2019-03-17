extern crate actix_web;
extern crate serde;
extern crate serde_json;

use actix_web::{fs::NamedFile, server, App, Error, HttpRequest, HttpResponse, Responder, Result};
use serde::Serialize;
use std::path::Path;

fn index(_req: &HttpRequest) -> Result<NamedFile> {
    let path = Path::new("./index.html");
    Ok(NamedFile::open(path)?)
}

#[derive(Serialize)]
struct Status {
    host_name: &'static str,
    host_ip: &'static str,
    host_mask: &'static str,
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

fn get_status(_req: &HttpRequest) -> impl Responder {
    Status {
        host_name: "***分行人民街分理处",
        host_ip: "10.0.8.8",
        host_mask: "255.255.255.0",
        gateway_ip: "10.0.8.1",
        server_ip: "10.0.0.8:8080",
        connect_status: "已连接",
        power_status: "正常",
    }
}

fn main() {
    server::new(|| {
        vec![
            App::new()
                .prefix("/api")
                .resource("/status", |r| r.f(get_status)),
            App::new().resource("/", |r| r.f(index)),
        ]
    })
    .bind("0.0.0.0:80")
    .unwrap()
    .run()
}
