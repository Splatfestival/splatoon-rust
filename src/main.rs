use std::env::current_dir;
use std::{env, fs};

use std::fs::File;
use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddrV4};
use chrono::Local;
use log::{error, info, trace};
use once_cell::sync::Lazy;
use rc4::{KeyInit, Rc4, StreamCipher};
use rc4::consts::U5;
use simplelog::{ColorChoice, CombinedLogger, Config, LevelFilter, TerminalMode, TermLogger, WriteLogger};
use crate::protocols::auth;
use crate::protocols::server::RMCProtocolServer;
use crate::prudp::socket::{Socket, SocketData};
use crate::prudp::packet::{PRUDPPacket, VirtualPort};
use crate::prudp::router::Router;
use crate::rmc::message::RMCMessage;
use crate::rmc::response::{RMCResponse, RMCResponseResult, send_response};
use crate::rmc::response::ErrorCode::{Core_InvalidIndex, Core_NotImplemented};

mod endianness;
mod prudp;
pub mod rmc;
mod protocols;

static AUTH_SERVER_PORT: Lazy<u16> = Lazy::new(||{
    env::var("AUTH_SERVER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000)
});

static OWN_IP: Lazy<Ipv4Addr> = Lazy::new(||{
    env::var("SERVER_IP")
        .ok()
        .and_then(|s| s.parse().ok())
        .expect("no public ip specified")
});

#[tokio::main]
async fn main() {
    CombinedLogger::init(
        vec![
            TermLogger::new(LevelFilter::Info, Config::default(), TerminalMode::Mixed, ColorChoice::Auto),
            WriteLogger::new(LevelFilter::max(), Config::default(), {
                fs::create_dir_all("log").unwrap();
                File::create(format!("log/{}.log", Local::now().to_rfc2822())).unwrap()
            })
        ]
    ).unwrap();

    dotenv::dotenv().ok();

    start_servers().await;
}

async fn auth_server_handle_rmc(packet: PRUDPPacket, rmc_message: RMCMessage){

}

async fn start_servers(){
    info!("starting auth server on {}:{}", *OWN_IP, *AUTH_SERVER_PORT);

    let (auth_server_router, auth_router_join) =
        Router::new(SocketAddrV4::new(*OWN_IP, *AUTH_SERVER_PORT)).await
            .expect("unable to startauth server");

    info!("setting up endpoints");

    // dont assign it to the name _ as that will make it drop right here and now
    let rmcserver = RMCProtocolServer::new(Box::new([
        Box::new(auth::protocol)
    ]));

    let mut _socket =
        Socket::new(
            auth_server_router.clone(),
            VirtualPort::new(1,10),
            "6f599f81",
            Box::new(|_|{
                Box::pin(
                    async move {
                        let rc4: Rc4<U5> = Rc4::new_from_slice( "CD&ML".as_bytes()).unwrap();
                        let cypher = Box::new(rc4);
                        let server_cypher: Box<dyn StreamCipher + Send + Sync> = cypher;

                        let rc4: Rc4<U5> = Rc4::new_from_slice( "CD&ML".as_bytes()).unwrap();
                        let cypher = Box::new(rc4);
                        let client_cypher: Box<dyn StreamCipher + Send + Sync> = cypher;

                        (true, (server_cypher, client_cypher))
                    }
                )
            }),
            Box::new(move |packet, socket, connection|{
                let rmcserver = rmcserver.clone();
                Box::pin(async move { rmcserver.process_message(packet, &socket, connection).await; })
            })
        ).await.expect("unable to create socket");

    auth_router_join.await.expect("auth server crashed")
}


#[cfg(test)]
mod test{
    use std::io::Cursor;
    use std::num::ParseIntError;
    use std::str::from_utf8;
    use hmac::digest::consts::U5;
    use rc4::{KeyInit, Rc4, StreamCipher};
    use crate::prudp::packet::PRUDPPacket;
    use crate::rmc;

    fn from_hex_stream(val: &str) -> Result<Vec<u8>, ParseIntError> {
        let res: Result<Vec<u8>, _> = val.as_bytes()
            .chunks_exact(2)
            .map(|c| from_utf8(c).expect("unable to convert back to string"))
            .map(|s| u8::from_str_radix(s, 16))
            .collect();

        res
    }

    #[tokio::test]
    async fn simulate_packets(){
        let val = from_hex_stream("ead001037d00afa1e200a5000200d9e4a4050368c18c6de4e2fb1cc40f0c020100768744db99f92c5005a061fd2a1df280cd64d5c1a565952c6befa607cbaf34661312b16db0fa6fccfb81e28b5a3a9bed02b49152bbc99cc112b7e29b9e45ec3d4b89df0fe71390883d9a927c264d07ada0de9cd28499e3ccdf3fd079e4a9848d4d783778c42da2af06106a7326634dc5bec5c3438ef18e30109839ffcc").expect("uuuuh");

        let mut packet = PRUDPPacket::new(&mut Cursor::new(&val)).expect("invalid packet");

        let mut rc4: Rc4<U5> =
            Rc4::new_from_slice("CD&ML".as_bytes().into()).expect("invalid key");

        rc4.apply_keystream(&mut packet.payload);

        println!("packet: {:?}", packet);

        let rmc_packet = rmc::message::RMCMessage::new(&mut Cursor::new(&packet.payload)).expect("unable to read message");

        let mut a = Cursor::new(&rmc_packet.rest_of_data);

        //let pid = rmc::structures::string::read(&mut a).expect("unable to read pid");
    }

    #[tokio::test]
    async fn simulate_packets_response(){
        let val = from_hex_stream("ead001032501a1af6200a500010013ffcdbc3a2ebc44efc6e38ea32a72b40201002e8644db19fe2a5005a2637d2a16f3b1fe5633037c1ed61c5aefad8afebdf2ff8600e9350fba1298b570c70f6dd647eac2d3faf0ab74ef761e2ee43dc10e249e5f91aed6813dcc04b3c707d9442b6e353b9b0b654e98f860fe5379c41d3c2a1874b7dd37ebf499e03bd2fd3e9a9203c0959feb760c38f504dcd0c9e99b17fd410657da4efa3e01c8a68ab3042d6d489788d5580778d32249cdf1fba8bf68cf4019d116ea7c580622ea1e3635139d91b44635d5e95b6c35b33898fdc0117fa6fc7162840d07a49f1e7089aa0ea65409a8ddeb2334449ba73a0ff7de462cf4a706a696de0f0521b84ae5a3f8587f3585d202d3cc0fb0451519c1b830b5e3cdd6de52e9add7325cbbf08a7c2f8b875934942b226703a22b4bc8931932dab055049051e4144b02").expect("uuuuh");

        let mut packet = PRUDPPacket::new(&mut Cursor::new(&val)).expect("invalid packet");

        let mut rc4: Rc4<U5> =
            Rc4::new_from_slice("CD&ML".as_bytes().into()).expect("invalid key");

        rc4.apply_keystream(&mut packet.payload);

        println!("packet: {:?}", packet);
    }
}