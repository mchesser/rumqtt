use rumqtt::{MqttClient, MqttOptions, QoS};
use std::{thread, time::Duration};

fn main() {
    pretty_env_logger::init();
    let mqtt_options = MqttOptions::new("test-pubsub2", "127.0.0.1", 1883).set_keep_alive(10);
    let (mut mqtt_client, notifications) = MqttClient::start(mqtt_options).unwrap();

   //mqtt_client.subscribe("hello/world", QoS::ExactlyOnce).unwrap();

    thread::spawn(move || {
        for i in 0..100 {
            let payload = format!("publish {}", i);
            thread::sleep(Duration::from_secs(1));
            mqtt_client.publish("hello/world", QoS::AtMostOnce, false, payload).unwrap();
        }
    });

    for notification in notifications {
        println!("{:?}", notification)
    }
}
