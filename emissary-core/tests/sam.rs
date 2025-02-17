// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use emissary_core::{
    router::Router, runtime::AddressBook, Config, Ntcp2Config, SamConfig, TransitConfig,
};
use emissary_util::runtime::tokio::Runtime;
use futures::StreamExt;
use rand::{thread_rng, RngCore};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};
use yosemite::{
    style::{Anonymous, Repliable, Stream},
    DestinationKind, Error, I2pError, ProtocolError, RouterApi, Session, SessionOptions,
};

use std::{fs::File, future::Future, io::Read, pin::Pin, sync::Arc, time::Duration};

async fn make_router(
    floodfill: bool,
    net_id: u8,
    routers: Vec<Vec<u8>>,
) -> (Router<Runtime>, Vec<u8>) {
    let config = Config {
        net_id: Some(net_id),
        floodfill,
        insecure_tunnels: true,
        allow_local: true,
        metrics: None,
        ntcp2: Some(Ntcp2Config {
            port: 0u16,
            iv: {
                let mut iv = [0u8; 16];
                thread_rng().fill_bytes(&mut iv);
                iv
            },
            key: {
                let mut key = [0u8; 32];
                thread_rng().fill_bytes(&mut key);
                key
            },
            host: Some("127.0.0.1".parse().unwrap()),
            publish: true,
        }),
        routers,
        samv3_config: Some(SamConfig {
            tcp_port: 0u16,
            udp_port: 0u16,
            host: "127.0.0.1".to_string(),
        }),
        transit: Some(TransitConfig {
            max_tunnels: Some(5000),
        }),
        ..Default::default()
    };

    Router::<Runtime>::new(config).await.unwrap()
}

#[tokio::test]
async fn generate_destination() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    // generate new destination and create new session using the destination
    let (_destination, private_key) = tokio::time::timeout(
        Duration::from_secs(5),
        RouterApi::new(sam_tcp).generate_destination(),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let _session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            destination: DestinationKind::Persistent { private_key },
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
}

#[tokio::test]
async fn streaming_works() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

    stream.write_all(b"goodbye, world!\n").await.unwrap();

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn repliable_datagrams_work() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp/udp ports and spawn them in the background
    let mut ports = Vec::<(u16, u16)>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;
        let addr_info = router.protocol_address_info();

        ports.push((
            addr_info.sam_tcp.unwrap().port(),
            addr_info.sam_udp.unwrap().port(),
        ));
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Repliable>::new(SessionOptions {
            samv3_tcp_port: ports[0].0,
            samv3_udp_port: ports[0].1,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Repliable>::new(SessionOptions {
            samv3_tcp_port: ports[1].0,
            samv3_udp_port: ports[1].1,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let handle = tokio::spawn(async move {
        let mut buffer = vec![0u8; 64];

        let (nread, from) =
            tokio::time::timeout(Duration::from_secs(10), session1.recv_from(&mut buffer))
                .await
                .expect("no timeout")
                .expect("to succeed");
        assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

        session1.send_to(b"goodbye, world!\n", &from).await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    session2.send_to(b"hello, world!\n", &dest).await.unwrap();

    let mut buffer = vec![0u8; 64];
    let (nread, _from) =
        tokio::time::timeout(Duration::from_secs(10), session2.recv_from(&mut buffer))
            .await
            .expect("no timeout")
            .expect("to succeed");

    assert_eq!(
        std::str::from_utf8(&buffer[..nread]),
        Ok("goodbye, world!\n"),
    );
    let _ = handle.await;
}

#[tokio::test]
async fn anonymous_datagrams_work() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp/udp ports and spawn them in the background
    let mut ports = Vec::<(u16, u16)>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;
        let addr_info = router.protocol_address_info();

        ports.push((
            addr_info.sam_tcp.unwrap().port(),
            addr_info.sam_udp.unwrap().port(),
        ));
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(60),
        Session::<Anonymous>::new(SessionOptions {
            samv3_tcp_port: ports[0].0,
            samv3_udp_port: ports[0].1,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest1 = session1.destination().to_owned();

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(60),
        Session::<Anonymous>::new(SessionOptions {
            samv3_tcp_port: ports[1].0,
            samv3_udp_port: ports[1].1,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest2 = session2.destination().to_owned();

    let handle = tokio::spawn(async move {
        let mut buffer = vec![0u8; 64];

        let nread = tokio::time::timeout(Duration::from_secs(30), session1.recv(&mut buffer))
            .await
            .expect("no timeout")
            .expect("to succeed");
        assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

        session1.send_to(b"goodbye, world!\n", &dest2).await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    session2.send_to(b"hello, world!\n", &dest1).await.unwrap();

    let mut buffer = vec![0u8; 64];
    let nread = tokio::time::timeout(Duration::from_secs(30), session2.recv(&mut buffer))
        .await
        .expect("no timeout")
        .expect("to succeed");

    assert_eq!(
        std::str::from_utf8(&buffer[..nread]),
        Ok("goodbye, world!\n"),
    );
    let _ = handle.await;
}

#[tokio::test]
async fn open_stream_to_self() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    let mut session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session.destination().to_owned();

    match tokio::time::timeout(Duration::from_secs(10), session.connect(&dest))
        .await
        .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::CantReachPeer))) => {}
        _ => panic!("unexpected result"),
    }
}

#[tokio::test]
async fn create_same_session_twice_transient() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    let session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session.destination().to_owned();

    match tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            destination: DestinationKind::Persistent { private_key: dest },
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::DuplicateDest))) => {}
        _ => panic!("should not succeed"),
    }
}

#[tokio::test]
async fn create_same_session_twice_persistent() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    // generate new destination and create new session using the destination
    let (_destination, private_key) = tokio::time::timeout(
        Duration::from_secs(5),
        RouterApi::new(sam_tcp).generate_destination(),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let _session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            destination: DestinationKind::Persistent {
                private_key: private_key.clone(),
            },
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    match tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            destination: DestinationKind::Persistent { private_key },
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::DuplicateDest))) => {}
        _ => panic!("should not succeed"),
    }
}

#[tokio::test]
async fn duplicate_session_id() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    let _session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            nickname: String::from("session_id"),
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    match tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            nickname: String::from("session_id"),
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::DuplicateId))) => {}
        _ => panic!("should not succeed"),
    }
}

#[tokio::test]
async fn stream_lots_of_data() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let (data, digest) = {
        let mut data = vec![0u8; 256 * 1024];
        thread_rng().fill_bytes(&mut data);

        let mut hasher = Sha256::new();
        hasher.update(&data);

        (data, hasher.finalize())
    };

    let handle = tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(&data).await.unwrap();

        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 256 * 1024];
    stream.read_exact(&mut buffer).await.unwrap();

    let mut hasher = Sha256::new();
    hasher.update(&buffer);
    assert_eq!(digest, hasher.finalize());

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn forward_stream() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            silent_forward: true,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        session1.forward(listener.local_addr().unwrap().port()).await.unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();

        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));
    stream.write_all(b"goodbye, world!\n").await.unwrap();

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn connect_to_inactive_destination() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create the sam router and fetch the random sam tcp port from the router
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_tcp = router.protocol_address_info().sam_tcp.unwrap().port();

    // spawn the router inte background and wait a moment for the network to boot
    tokio::spawn(async move { while let Some(_) = router.next().await {} });
    tokio::time::sleep(Duration::from_secs(15)).await;

    // generate new destination and create new session using the destination
    let (destination, _private_key) = tokio::time::timeout(
        Duration::from_secs(5),
        RouterApi::new(sam_tcp).generate_destination(),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut session = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_tcp,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    match tokio::time::timeout(Duration::from_secs(60), session.connect(&destination))
        .await
        .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::CantReachPeer))) => {}
        _ => panic!("unexpected result"),
    }
}

#[tokio::test]
async fn closed_stream_detected() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );
        stream.shutdown().await.unwrap();
        drop(stream);
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

    stream.write_all(b"goodbye, world!\n").await.unwrap();

    match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
        .await
        .expect("no timeout")
    {
        Ok(0) => {}
        _ => panic!("unexpected result"),
    }

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn close_and_reconnect() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        for _ in 0..2 {
            let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
                .await
                .expect("no timeout")
                .expect("to succeed");

            stream.write_all(b"hello, world!\n").await.unwrap();

            let mut buffer = vec![0u8; 64];
            let nread = stream.read(&mut buffer).await.unwrap();
            assert_eq!(
                std::str::from_utf8(&buffer[..nread]),
                Ok("goodbye, world!\n")
            );
            stream.shutdown().await.unwrap();
            drop(stream);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    for _ in 0..2 {
        let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
            .await
            .expect("no timeout")
            .expect("to succeed");

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();

        assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

        stream.write_all(b"goodbye, world!\n").await.unwrap();

        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
            .await
            .expect("no timeout")
        {
            Ok(0) => {}
            _ => panic!("unexpected result"),
        }
    }

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn create_multiple_sessions() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..6 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let port = router.protocol_address_info().sam_tcp.unwrap().port();
    tokio::spawn(async move { while let Some(_) = router.next().await {} });

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let stream = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: port,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let repliable = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Repliable>::new(SessionOptions {
            samv3_tcp_port: port,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    drop(stream);
    drop(repliable);

    tokio::time::sleep(Duration::from_secs(2)).await;

    let _anonymous = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Anonymous>::new(SessionOptions {
            samv3_tcp_port: port,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
}

#[tokio::test]
async fn send_data_to_destroyed_session() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(20)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(session1);
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect(&dest))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

    let future = async move {
        loop {
            match stream.write_all(b"goodbye, world!\n").await {
                Ok(_) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(_) => break,
            }
        }
    };

    tokio::time::timeout(Duration::from_secs(15), future).await.expect("no timeout");
}

#[tokio::test]
async fn connect_using_b32_i2p() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    let private_key = {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test-vectors/destination.b64");
        let mut file = File::open(path).unwrap();
        let mut private_key = String::new();

        file.read_to_string(&mut private_key).unwrap();
        private_key
    };

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            destination: DestinationKind::Persistent { private_key },
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let handle = tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(15), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
        session2.connect("2yatlfcp76l6x2y3w2jt27d5gn4cwpdjrfudv2y3dvqgghklfzfq.b32.i2p"),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

    stream.write_all(b"goodbye, world!\n").await.unwrap();

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn unpublished_destination() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..4 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create two more routers, fetch their sam tcp ports and spawn them in the background
    let mut ports = Vec::<u16>::new();

    for _ in 0..2 {
        let mut router = make_router(false, net_id, router_infos.clone()).await.0;

        ports.push(router.protocol_address_info().sam_tcp.unwrap().port());
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[0],
            publish: false,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(15), session1.accept()).await {
            Err(_) => {}
            _ => panic!("unexpected success"),
        }
    });

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: ports[1],
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    match tokio::time::timeout(Duration::from_secs(60), session2.connect(&dest))
        .await
        .expect("no timeout")
    {
        Err(Error::Protocol(ProtocolError::Router(I2pError::CantReachPeer))) => {}
        _ => panic!("unexpected result"),
    }

    assert!(handle.await.is_ok());
}

#[tokio::test]
async fn host_lookup() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let mut router_infos = Vec::<Vec<u8>>::new();
    let net_id = (thread_rng().next_u32() % 255) as u8;

    for i in 0..6 {
        let (mut router, router_info) = make_router(i < 2, net_id, router_infos.clone()).await;

        router_infos.push(router_info);
        tokio::spawn(async move { while let Some(_) = router.next().await {} });
    }

    // create router for the sam server
    let mut router = make_router(false, net_id, router_infos.clone()).await.0;
    let sam_port = router.protocol_address_info().sam_tcp.unwrap().port();
    tokio::spawn(async move { while let Some(_) = router.next().await {} });

    // let the network boot up
    tokio::time::sleep(Duration::from_secs(40)).await;

    let mut session1 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_port,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");
    let dest = session1.destination().to_owned();

    let handle = tokio::spawn(async move {
        let mut stream = tokio::time::timeout(Duration::from_secs(120), session1.accept())
            .await
            .expect("no timeout")
            .expect("to succeed");

        stream.write_all(b"hello, world!\n").await.unwrap();

        let mut buffer = vec![0u8; 64];
        let nread = stream.read(&mut buffer).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&buffer[..nread]),
            Ok("goodbye, world!\n")
        );

        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let config = Config {
        net_id: Some(net_id),
        floodfill: false,
        insecure_tunnels: true,
        allow_local: true,
        metrics: None,
        ntcp2: Some(Ntcp2Config {
            port: 0u16,
            iv: {
                let mut iv = [0u8; 16];
                thread_rng().fill_bytes(&mut iv);
                iv
            },
            key: {
                let mut key = [0u8; 32];
                thread_rng().fill_bytes(&mut key);
                key
            },
            host: Some("127.0.0.1".parse().unwrap()),
            publish: true,
        }),
        routers: router_infos.clone(),
        samv3_config: Some(SamConfig {
            tcp_port: 0u16,
            udp_port: 0u16,
            host: "127.0.0.1".to_string(),
        }),
        ..Default::default()
    };

    struct AddressBookImpl {
        dest: String,
    }

    impl AddressBook for AddressBookImpl {
        fn resolve(&self, _: String) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> {
            let dest = self.dest.clone();
            Box::pin(async move { Some(dest) })
        }
    }

    let (mut router, _) =
        Router::<Runtime>::with_address_book(config, Arc::new(AddressBookImpl { dest }))
            .await
            .unwrap();
    let sam_port = router.protocol_address_info().sam_tcp.unwrap().port();
    tokio::spawn(async move { while let Some(_) = router.next().await {} });

    tokio::time::sleep(Duration::from_secs(30)).await;

    let mut session2 = tokio::time::timeout(
        Duration::from_secs(30),
        Session::<Stream>::new(SessionOptions {
            samv3_tcp_port: sam_port,
            ..Default::default()
        }),
    )
    .await
    .expect("no timeout")
    .expect("to succeed");

    let mut stream = tokio::time::timeout(Duration::from_secs(10), session2.connect("host.i2p"))
        .await
        .expect("no timeout")
        .expect("to succeed");

    let mut buffer = vec![0u8; 64];
    let nread = stream.read(&mut buffer).await.unwrap();

    assert_eq!(std::str::from_utf8(&buffer[..nread]), Ok("hello, world!\n"));

    stream.write_all(b"goodbye, world!\n").await.unwrap();

    assert!(handle.await.is_ok());
}
