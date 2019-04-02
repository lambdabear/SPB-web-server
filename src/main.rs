use rand::prelude::*;
use rumqtt::MqttClient;
use spb_serial_data_parser::{Battery, DcOut, SwIn, SwOut, Ups};
use spb_web_server::*;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() {
    // setup mqtt client
    // let broker = "test.mosquitto.org";
    // let port = 1883;
    // let id = "spb001";
    // let topic = "hello/world";
    // let (mut mqtt_client, _notifications) = setup_client_loop(broker, port, id);
    // mqtt_client.subscribe(topic, QoS::AtLeastOnce).unwrap();

    let mqtt_c: Arc<Mutex<Option<MqttClient>>> = Arc::new(Mutex::new(None));
    let c1 = mqtt_c.clone();

    // spb data cache init
    let swin = Arc::new(Mutex::new(SwIn::new(false, false, false, false)));
    let swout = Arc::new(Mutex::new(SwOut::new(
        false, false, false, false, false, false,
    )));
    let ups = Arc::new(Mutex::new(Ups::new(0.0, 0.0, 0.0, false)));
    let bt = Arc::new(Mutex::new(Battery::new(0.0, 0.0, 0)));
    let dc = Arc::new(Mutex::new(DcOut::new(0.0, 0.0, 0.0)));

    let in1 = swin.clone();

    let h1 = thread::spawn(move || {
        run(swin, swout, ups, bt, dc, mqtt_c);
    });

    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(5000));
        match c1.lock() {
            Ok(c) => match *c {
                Some(_) => (),
                None => println!("mqtt client is not setup"),
            },
            Err(e) => eprintln!("{}", e),
        }
        match in1.lock() {
            Ok(mut swin) => {
                *swin = SwIn::new(rand::random(), random(), random(), random());
            }
            Err(e) => eprintln!("{:?}", e),
        }
    });

    h1.join().unwrap();
}
