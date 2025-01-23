use std::array;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::ops::Deref;
use tokio::net::UdpSocket;
use std::sync::{Arc};
use tokio::sync::{Mutex, RwLock};
use hmac::{Hmac, Mac};
use log::{error, info, trace, warn};
use rand::random;
use rc4::consts::U256;
use rustls::internal::msgs::handshake::SessionId;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use crate::prudp::packet::{flags, PacketOption, PRUDPPacket, types, VirtualPort};
use crate::prudp::packet::flags::{ACK, HAS_SIZE, MULTI_ACK, NEED_ACK, RELIABLE};
use crate::prudp::packet::PacketOption::{ConnectionSignature, MaximumSubstreamId, SupportedFunctions};
use crate::prudp::packet::types::{CONNECT, DATA, SYN};
use crate::prudp::router::{Error, Router};
use crate::prudp::sockaddr::PRUDPSockAddr;


/// PRUDP Socket for accepting connections to then send and recieve data from those clients
pub struct Socket(Arc<SocketImpl>, Arc<Router>, Receiver<Connection>);

#[derive(Debug)]
pub struct SocketImpl {
    virtual_port: VirtualPort,
    socket: Arc<UdpSocket>,
    access_key: &'static str,
    connections: RwLock<HashMap<PRUDPSockAddr, Arc<Mutex<Connection>>>>,
    connection_creation_sender: Sender<Connection>,
}



#[derive(Debug)]
pub struct Connection {
    sock_addr: PRUDPSockAddr,
    id: u64,
    signature: [u8; 16],
    server_signature: [u8; 16],
    session_id: u8,
    reliable_client_counter: u16,
    reliable_server_counter: u16,
    reliable_client_queue: VecDeque<PRUDPPacket>,
}


impl Socket {
    pub async fn new(router: Arc<Router>, port: VirtualPort, access_key: &'static str) -> Result<Self, Error> {
        trace!("creating socket on router at {} on virtual port {:?}", router.get_own_address(), port);
        let (send, recv) = channel(20);

        let socket = Arc::new(
            SocketImpl::new(&router, send, port, access_key)
        );

        router.add_socket(socket.clone()).await?;

        Ok(Self(socket, router, recv))
    }

    pub async fn accept(&mut self) -> Option<Connection> {
        self.2.recv().await
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        {
            let router = self.1.clone();

            let virtual_port = self.virtual_port;
            trace!("socket dropped socket will be removed from router soon");
            // it's not that important to remove it immediately so we can delay the deletion a bit if needed
            tokio::spawn(async move {
                router.remove_socket(virtual_port).await;
                trace!("socket removed from router successfully");
            });
        }
    }
}

impl Deref for Socket {
    type Target = SocketImpl;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}


impl SocketImpl {
    fn new(router: &Router, connection_creation_sender: Sender<Connection>, port: VirtualPort, access_key: &'static str) -> Self {
        SocketImpl {
            socket: router.get_udp_socket(),
            virtual_port: port,
            connections: Default::default(),
            access_key,
            connection_creation_sender,
        }
    }

    pub fn get_virual_port(&self) -> VirtualPort {
        self.virtual_port
    }

    pub async fn process_packet(&self, connection: PRUDPSockAddr, packet: &PRUDPPacket) {
        info!("recieved packet on endpoint");

        let conn = self.connections.read().await;

        if !conn.contains_key(&connection) {
            drop(conn);

            let mut conn = self.connections.write().await;
            //only insert if we STILL dont have the connection preventing double insertion
            if !conn.contains_key(&connection) {
                conn.insert(connection, Arc::new(Mutex::new(Connection {
                    sock_addr: connection,
                    id: random(),
                    signature: [0; 16],
                    server_signature: [0; 16],
                    session_id: 0,
                    reliable_client_queue: VecDeque::new(),
                    reliable_client_counter: 0,
                    reliable_server_counter: 0,
                })));
            }
            drop(conn);
        } else {
            drop(conn);
        }

        let connections = self.connections.read().await;

        let Some(conn) = connections.get(&connection) else {
            error!("connection is still not present after making sure connection is present, giving up.");
            return;
        };

        let conn = conn.clone();

        // dont keep holding the connections list unnescesarily
        drop(connections);

        let mut conn = conn.lock().await;

        if (packet.header.types_and_flags.get_flags() & ACK) != 0 {
            info!("acknowledgement recieved");
            return;
        }

        if (packet.header.types_and_flags.get_flags() & MULTI_ACK) != 0 {
            info!("acknowledgement recieved");
            unimplemented!()
        }


        match packet.header.types_and_flags.get_types() {
            SYN => {
                info!("got syn");
                // reset heartbeat?
                let mut response_packet = packet.base_response_packet();

                response_packet.header.types_and_flags.set_types(SYN);
                response_packet.header.types_and_flags.set_flag(ACK);
                response_packet.header.types_and_flags.set_flag(HAS_SIZE);

                conn.signature = connection.calculate_connection_signature();

                response_packet.options.push(ConnectionSignature(conn.signature));

                for options in &packet.options {
                    match options {
                        SupportedFunctions(functions) => {
                            response_packet.options.push(SupportedFunctions(*functions & 0x04))
                        }
                        MaximumSubstreamId(max_substream) => {
                            response_packet.options.push(MaximumSubstreamId(*max_substream))
                        }
                        _ => { /* ??? */ }
                    }
                }

                response_packet.set_sizes();

                response_packet.calculate_and_assign_signature(self.access_key, None, None);

                let mut vec = Vec::new();

                response_packet.write_to(&mut vec).expect("somehow failed to convert backet to bytes");

                self.socket.send_to(&vec, connection.regular_socket_addr).await.expect("failed to send data back");
            }
            CONNECT => {
                info!("got connect");

                let mut response_packet = packet.base_response_packet();

                response_packet.header.types_and_flags.set_types(CONNECT);
                response_packet.header.types_and_flags.set_flag(ACK);
                response_packet.header.types_and_flags.set_flag(HAS_SIZE);

                // todo: (or not) sliding windows and stuff
                conn.session_id = packet.header.session_id;
                response_packet.header.session_id = conn.session_id;
                response_packet.header.sequence_id = 1;

                response_packet.options.push(ConnectionSignature(Default::default()));

                for option in &packet.options {
                    match option {
                        MaximumSubstreamId(max_substream) => response_packet.options.push(MaximumSubstreamId(*max_substream)),
                        SupportedFunctions(funcs) => response_packet.options.push(SupportedFunctions(*funcs)),
                        ConnectionSignature(sig) => {
                            conn.server_signature = *sig
                        }
                        _ => { /* ? */ }
                    }
                }

                // Splatoon doesnt use compression so we arent gonna compress unless i at some point
                // want to implement some server which requires it
                // No encryption here for the same reason

                // todo: implement something to do secure servers

                if conn.server_signature == <[u8; 16] as Default>::default() {
                    error!("didn't get connection signature from client")
                }

                response_packet.set_sizes();

                response_packet.calculate_and_assign_signature(self.access_key, None, Some(conn.server_signature));

                let mut vec = Vec::new();
                response_packet.write_to(&mut vec).expect("somehow failed to convert backet to bytes");

                self.socket.send_to(&vec, connection.regular_socket_addr).await.expect("failed to send data back");
            }
            DATA => {
                if (packet.header.types_and_flags.get_flags() & RELIABLE) != 0 {
                    match conn.reliable_client_queue.binary_search_by_key(&conn.reliable_client_counter, |p| p.header.sequence_id) {
                        Ok(_) => warn!("recieved packet twice"),
                        Err(position) => conn.reliable_client_queue.insert(position, packet.clone()),
                    }

                    if (packet.header.types_and_flags.get_flags() & NEED_ACK) != 0{
                        let mut ack = packet.base_acknowledgement_packet();
                        ack.set_sizes();
                        ack.calculate_and_assign_signature(self.access_key, None, Some(conn.server_signature));

                        let mut vec = Vec::new();
                        ack.write_to(&mut vec).expect("somehow failed to convert backet to bytes");

                        self.socket.send_to(&vec, connection.regular_socket_addr).await.expect("failed to send data back");
                    }

                    while let Some(packet) =
                        conn.reliable_client_queue
                            .front()
                            .is_some_and(|v| v.header.sequence_id == conn.reliable_client_counter)
                            .then(|| conn.reliable_client_queue.pop_front())
                            .flatten(){
                        conn.reliable_client_counter = conn.reliable_client_counter.overflowing_add(1).0;

                        // ignored
                    }
                } else {
                    error!("unreliable packets are unimplemented");
                    unimplemented!()
                }
                info!("{:?}", packet);
            }
            _ => unimplemented!("unimplemented packet type: {}", packet.header.types_and_flags.get_types())
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::Cursor;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::sync::Arc;
    use tokio::net::UdpSocket;
    use tokio::sync::mpsc::channel;
    use crate::prudp::packet::{PRUDPPacket, VirtualPort};
    use crate::prudp::sockaddr::PRUDPSockAddr;
    use crate::prudp::socket::SocketImpl;

    #[tokio::test]
    async fn test_connect() {
        let packet_1 = [234, 208, 1, 27, 0, 0, 175, 161, 192, 0, 0, 0, 0, 0, 36, 21, 233, 179, 203, 154, 57, 222, 219, 9, 21, 2, 29, 172, 56, 92, 0, 4, 4, 1, 0, 0, 1, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 1, 0];
        let packet_2 = [234, 208, 1, 31, 0, 0, 175, 161, 225, 0, 249, 0, 1, 0, 40, 168, 31, 138, 58, 193, 30, 134, 3, 232, 205, 245, 28, 155, 193, 198, 0, 4, 0, 0, 0, 0, 1, 16, 211, 240, 113, 188, 227, 114, 114, 30, 157, 179, 246, 55, 233, 240, 44, 197, 3, 2, 247, 244, 4, 1, 0];

        let packet_1 = PRUDPPacket::new(&mut Cursor::new(packet_1)).unwrap();
        let packet_2 = PRUDPPacket::new(&mut Cursor::new(packet_2)).unwrap();


        let (send, recv) = channel(100);

        let sock = SocketImpl {
            connections: Default::default(),
            access_key: "6f599f81",
            virtual_port: VirtualPort(0),
            socket: Arc::new(UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 10000)).await.unwrap()),
            connection_creation_sender: send,
        };
        println!("sent: {:?}", packet_1);
        sock.process_packet(PRUDPSockAddr {
            virtual_port: VirtualPort(0),
            regular_socket_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2469),
        }, &packet_1).await;
        println!("sent: {:?}", packet_2);
        sock.process_packet(PRUDPSockAddr {
            virtual_port: VirtualPort(0),
            regular_socket_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2469),
        }, &packet_2).await;
    }
}