use std::io::{self, Read, Write};

use crate::client::network::stream::NetworkStream;
use futures::Poll;
use serde_derive::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio_io::{AsyncRead, AsyncWrite};

#[cfg(feature = "rustls")]
pub mod stream {
use crate::client::network::{generate_httpproxy_auth, lookup_ipv4};
    use crate::codec::MqttCodec;
    use crate::error::ConnectError;
    use futures::{
        future::{self, Either},
        sink::Sink,
        stream::Stream,
        Future,
    };
    use std::{
        io::{
            self, {BufReader, Cursor},
        },
        sync::Arc,
    };
    use tokio::{io::AsyncRead, net::TcpStream};
    use tokio_codec::{Decoder, Framed, LinesCodec};
    use tokio_rustls::{
        rustls::{internal::pemfile, ClientConfig, ClientSession},
        TlsConnector, TlsStream,
    };
    use webpki::DNSNameRef;

    pub enum NetworkStream {
        Tcp(TcpStream),
        Tls(TlsStream<TcpStream, ClientSession>),
    }

    impl NetworkStream {
        pub fn builder() -> NetworkStreamBuilder {
            NetworkStreamBuilder {
                certificate_authority: None,
                client_cert: None,
                client_private_key: None,
                http_proxy: None,
            }
        }
    }

    #[derive(Clone)]
    struct HttpProxy {
        id: String,
        proxy_host: String,
        proxy_port: u16,
        key: Vec<u8>,
        expiry: i64
    }

    pub struct NetworkStreamBuilder {
        certificate_authority: Option<Vec<u8>>,
        client_cert: Option<Vec<u8>>,
        client_private_key: Option<Vec<u8>>,
        http_proxy: Option<HttpProxy>,
    }

    impl NetworkStreamBuilder {
        pub fn add_certificate_authority(mut self, ca: &[u8]) -> NetworkStreamBuilder {
            self.certificate_authority = Some(ca.to_vec());
            self
        }

        pub fn add_client_auth(mut self, cert: &[u8], private_key: &[u8]) -> NetworkStreamBuilder {
            self.client_cert = Some(cert.to_vec());
            self.client_private_key = Some(private_key.to_vec());
            self
        }

        pub fn set_http_proxy(
            mut self,
            id: &str,
            proxy_host: &str,
            proxy_port: u16,
            key: &[u8],
            expiry: i64,
        ) -> NetworkStreamBuilder {
            self.http_proxy = Some(HttpProxy {
                id: id.to_owned(),
                proxy_host: proxy_host.to_owned(),
                proxy_port: proxy_port,
                key: key.to_owned(),
                expiry
            });

            self
        }

        fn create_stream(&mut self) -> Result<TlsConnector, ConnectError> {
            let mut config = ClientConfig::new();

            match self.certificate_authority.clone() {
                Some(ca) => {
                    let mut ca = BufReader::new(Cursor::new(ca));
                    config.root_store.add_pem_file(&mut ca).unwrap();
                }
                None => return Err(ConnectError::NoCertificateAuthority),
            }

            match (self.client_cert.clone(), self.client_private_key.clone()) {
                (Some(cert), Some(key)) => {
                    let mut cert = BufReader::new(Cursor::new(cert));
                    let mut keys = BufReader::new(Cursor::new(key));

                    let certs = pemfile::certs(&mut cert).unwrap();
                    let keys = pemfile::rsa_private_keys(&mut keys).unwrap();

                    config.set_single_client_cert(certs, keys[0].clone());
                }
                (None, None) => (),
                _ => unimplemented!(),
            };

            Ok(TlsConnector::from(Arc::new(config)))
        }

        pub fn http_connect(
            &self,
            id: &str,
            proxy_host: &str,
            proxy_port: u16,
            host: &str,
            port: u16,
            key: &[u8],
            expiry: i64,
        ) -> impl Future<Item = TcpStream, Error = io::Error> {
            let proxy_auth = generate_httpproxy_auth(id, key, expiry);
            let connect = format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\nProxy-Authorization: {}\r\n\r\n",
                host, port, host, port, proxy_auth
            );
            debug!("{}", connect);

            let codec = LinesCodec::new();
            let addr = lookup_ipv4(proxy_host, proxy_port);
            let addr = future::result(addr);

            addr.and_then(|proxy_address| TcpStream::connect(&proxy_address))
                .and_then(|tcp| {
                    let framed = tcp.framed(codec);
                    future::ok(framed)
                })
                .and_then(|f| f.send(connect))
                .and_then(|f| f.into_future().map_err(|(e, _f)| e))
                .and_then(|(s, f)| {
                    debug!("{:?}", s);
                    f.into_future().map_err(|(e, _f)| e)
                })
                .and_then(|(s, f)| {
                    debug!("{:?}", s);
                    f.into_future().map_err(|(e, _f)| e)
                })
                .and_then(|(s, f)| {
                    debug!("{:?}", s);
                    let stream = f.into_inner();
                    future::ok(stream)
                })
        }

        pub fn tcp_connect(&self, host: &str, port: u16) -> impl Future<Item = TcpStream, Error = io::Error> {
            let addr = lookup_ipv4(host, port);
            let addr = future::result(addr);

            addr.and_then(|addr| {
                TcpStream::connect(&addr)
            })
        }

        pub fn connect(
            mut self,
            host: &str,
            port: u16,
        ) -> impl Future<Item = Framed<NetworkStream, MqttCodec>, Error = ConnectError> {
            let tls_connector = self.create_stream();
            let host_tcp = host.to_owned();
            let http_proxy = self.http_proxy.clone();
            let stream = match http_proxy {
                Some(HttpProxy{id, proxy_host, proxy_port, key, expiry}) => {
                    let s = self.http_connect(&id, &proxy_host, proxy_port, &host_tcp, port, &key, expiry);
                    Either::A(s)
                }
                None => {
                    let s = self.tcp_connect(host, port);
                    Either::B(s)
                }
            };

            match tls_connector {
                Ok(tls_connector) => {
                    let domain = DNSNameRef::try_from_ascii_str(&host).unwrap().to_owned();
                    Either::A(
                        stream
                            .and_then(move |stream| tls_connector.connect(domain.as_ref(), stream))
                            .map_err(ConnectError::from)
                            .and_then(|stream| {
                                let stream = NetworkStream::Tls(stream);
                                future::ok(MqttCodec.framed(stream))
                            }),
                    )
                }
                Err(ConnectError::NoCertificateAuthority) => Either::B(
                    stream
                        .and_then(|stream| {
                            let stream = NetworkStream::Tcp(stream);
                            future::ok(MqttCodec.framed(stream))
                        })
                        .map_err(ConnectError::from),
                ),
                _ => unimplemented!(),
            }
        }
    }
}

#[cfg(feature = "nativetls")]
mod stream {
    use tokio::net::TcpStream;
    use tokio_tls::TlsStream;

    pub enum NetworkStream {
        Tcp(TcpStream),
        Tls(TlsStream<TcpStream>),
    }

    impl NetworkStream {}
}

#[cfg(not(any(feature = "rustls", feature = "nativetls")))]
pub mod stream {
    #![allow(unused_variables, unused_mut)]

    use crate::client::network::lookup_ipv4;
    use crate::codec::MqttCodec;
    use crate::error::ConnectError;
    use futures::{future::self, stream::Stream, Future};
    use std::io;
    use tokio::{io::AsyncRead, net::TcpStream};
    use tokio_codec::{Decoder, Framed};

    pub enum NetworkStream {
        Tcp(TcpStream),
        Tls(TcpStream),
    }

    impl NetworkStream {
        pub fn builder() -> NetworkStreamBuilder {
            NetworkStreamBuilder {}
        }
    }

    pub struct NetworkStreamBuilder {
    }

    impl NetworkStreamBuilder {
        pub fn add_certificate_authority(mut self, ca: &[u8]) -> NetworkStreamBuilder {
            unimplemented!()
        }

        pub fn add_client_auth(mut self, cert: &[u8], private_key: &[u8]) -> NetworkStreamBuilder {
            unimplemented!()
        }

        pub fn set_http_proxy(
            mut self,
            id: &str,
            proxy_host: &str,
            proxy_port: u16,
            key: &[u8],
            expiry: i64,
        ) -> NetworkStreamBuilder {
            unimplemented!()
        }

        pub fn http_connect(
            &self,
            id: &str,
            proxy_host: &str,
            proxy_port: u16,
            host: &str,
            port: u16,
            key: &[u8],
            expiry: i64,
        ) -> ! {
            unimplemented!()
        }

        pub fn tcp_connect(&self, host: &str, port: u16) -> impl Future<Item = TcpStream, Error = io::Error> {
            let addr = lookup_ipv4(host, port);
            TcpStream::connect(&addr)
        }

        pub fn connect(
            mut self,
            host: &str,
            port: u16,
        ) -> impl Future<Item = Framed<NetworkStream, MqttCodec>, Error = ConnectError> {
            self.tcp_connect(host, port)
                .and_then(|stream| {
                    let stream = NetworkStream::Tcp(stream);
                    future::ok(MqttCodec.framed(stream))
                })
                .map_err(ConnectError::from)
        }
    }
}

fn lookup_ipv4(host: &str, port: u16) -> Result<SocketAddr, io::Error> {
    use std::net::ToSocketAddrs;

    let addrs = (host, port).to_socket_addrs()?;
    for addr in addrs {
        if let SocketAddr::V4(_) = addr {
            return Ok(addr);
        }
    }

    unreachable!("Cannot lookup address");
}

#[cfg(any(feature = "rustls", feature = "nativetls"))]
fn generate_httpproxy_auth(id: &str, key: &[u8], expiry: i64) -> String {
    use chrono::{Duration, Utc};
    use jsonwebtoken::{encode, Algorithm, Header};
    use uuid::Uuid;

    #[derive(Debug, Serialize, Deserialize)]
    struct Claims {
        iat: i64,
        exp: i64,
        jti: Uuid,
    }

    let time = Utc::now();
    let jwt_header = Header::new(Algorithm::RS256);
    let iat = time.timestamp();
    let jti = Uuid::new_v4();

    let exp = time
        .checked_add_signed(Duration::minutes(expiry))
        .expect("Unable to create expiry")
        .timestamp();

    let claims = Claims { iat, exp, jti };

    let jwt = encode(&jwt_header, &claims, &key).unwrap();
    let userid_password = format!("{}:{}", id, jwt);
    let auth = base64::encode(userid_password.as_bytes());

    format!("Basic {}", auth)
}

impl Read for NetworkStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            NetworkStream::Tcp(ref mut s) => s.read(buf),
            NetworkStream::Tls(ref mut s) => s.read(buf),
        }
    }
}

impl Write for NetworkStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            NetworkStream::Tcp(ref mut s) => s.write(buf),
            NetworkStream::Tls(ref mut s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            NetworkStream::Tcp(ref mut s) => s.flush(),
            NetworkStream::Tls(ref mut s) => s.flush(),
        }
    }
}

impl AsyncRead for NetworkStream {}
impl AsyncWrite for NetworkStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        match *self {
            NetworkStream::Tcp(ref mut s) => s.shutdown(),
            NetworkStream::Tls(ref mut s) => s.shutdown(),
        }
    }
}
