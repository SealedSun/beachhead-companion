use ::common::{Config,self};
use redis::{self, Commands};

// The Redis server handling code is taken from the redis-rs project itself.
// https://github.com/mitsuhiko/redis-rs/blob/master/tests/test_basic.rs
// See LICENSE for a copy of the license
extern crate net2;

use std::process;
use std::thread::{sleep};
use std::time::Duration;
use std::rc::Rc;

pub struct RedisServer {
    pub process: process::Child,
    addr: redis::ConnectionAddr,
}

#[allow(unused)]
impl RedisServer {

    pub fn new() -> RedisServer {
        let mut cmd = process::Command::new("redis-server");
        cmd
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::null());

        // this is technically a race but we can't do better with
        // the tools that redis gives us :(
        let listener = net2::TcpBuilder::new_v4().unwrap()
            .reuse_address(true).unwrap()
            .bind("127.0.0.1:0").unwrap()
            .listen(1).unwrap();
        let server_port = listener.local_addr().unwrap().port();
        cmd
            .arg("--port").arg(server_port.to_string())
            .arg("--bind").arg("127.0.0.1");
        let addr = redis::ConnectionAddr::Tcp("127.0.0.1".to_string(), server_port);

        let process = cmd.spawn().unwrap();
        RedisServer {
            process: process,
            addr: addr,
        }
    }

    pub fn wait(&mut self) {
        self.process.wait().unwrap();
    }

    pub fn get_client_addr(&self) -> &redis::ConnectionAddr {
        &self.addr
    }

    pub fn configure(&self, config: &mut Config) {
        let addr = self.get_client_addr();
        if let &redis::ConnectionAddr::Tcp(ref host, port) = addr {
            config.redis_host = Rc::new(host.to_owned());
            config.redis_port = port;
        } else {
            panic!("Expected TCP address, got {:?}", addr);
        }
    }
}

impl Drop for RedisServer {

    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

pub struct TestContext {
    pub server: RedisServer,
    pub client: redis::Client,
}

#[allow(unused)]
impl TestContext {

    fn new() -> TestContext {
        let server = RedisServer::new();

        let client = redis::Client::open(redis::ConnectionInfo {
            addr: Box::new(server.get_client_addr().clone()),
            db: 0,
            passwd: None,
        }).unwrap();
        let con;

        let millisecond = Duration::from_millis(1);
        loop {
            match client.get_connection() {
                Err(err) => {
                    if err.is_connection_refusal() {
                        sleep(millisecond);
                    } else {
                        panic!("Could not connect: {}", err);
                    }
                },
                Ok(x) => { con = x; break; },
            }
        }
        redis::cmd("FLUSHDB").execute(&con);

        TestContext {
            server: server,
            client: client,
        }
    }

    fn connection(&self) -> redis::Connection {
        self.client.get_connection().unwrap()
    }

    fn pubsub(&self) -> redis::PubSub {
        self.client.get_pubsub().unwrap()
    }
}

// end of redis-rs derived code

// I initially wanted this code to live in an integration test (./tests) but that would mean
// not using or re-implementing all of the testing infrastructure (mocks etc.)

use publisher::{Publication};
use domain_spec::{DomainSpec};
use rustc_serialize::json::{self, Json};
use std::sync::Arc;
use ::publisher::Publish;

#[test]
fn test_hostonly() {
    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let mut service_key = (*config.key_prefix).to_owned();
    service_key.push_str("example.com");

    // #### WHEN  ####
    redis_publisher.publish(&Publication {
        host: "example.com".to_owned(),
        specs: Vec::new()
    }).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys : Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rawpub : Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(), "top-level value stored in redis must be an array, was {:?}",
        rawpub);
    let specs : &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 0, "zero domain specs expected, got {:?}", specs);
}