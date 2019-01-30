use rumqtt::{ConnectionMethod, MqttClient, MqttOptions, QoS};
use std::{thread, time::Duration};

fn main() {
    pretty_env_logger::init();

    let client_id = "tls-test".to_owned();
    let ca = include_bytes!("tlsfiles/ca-chain.cert.pem").to_vec();
    let client_cert = include_bytes!("tlsfiles/bike1.cert.pem").to_vec();
    let client_key = include_bytes!("tlsfiles/bike1.key.pem").to_vec();

    let connection_method = ConnectionMethod::Tls(ca, Some((client_cert, client_key)));

    let mqtt_options = MqttOptions::new(client_id, "localhost", 8883)
        .set_keep_alive(10)
        .set_connection_method(connection_method);

    let (mut mqtt_client, notifications) = MqttClient::start(mqtt_options).unwrap();
    let topic = "hello/world";

    thread::spawn(move || {
        for i in 0..100 {
            let payload = format!("publish {}", i);
            thread::sleep(Duration::from_secs(1));
            mqtt_client.publish(topic.clone(), QoS::AtLeastOnce, false, payload).unwrap();
        }
    });

    for notification in notifications {
        println!("{:?}", notification)
    }
}
