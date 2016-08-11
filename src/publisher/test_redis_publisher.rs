use ::common::{self, Config};
use redis::{self, Commands};

// In case we want to throw a lock around redis server creation. Reduces risk of address-in-use
// problems, but slows down test execution.
// use std::sync::Mutex;
// lazy_static! {
//    static ref REDIS_START_LOCK : Mutex<()> = Mutex::new(());
// }
// let lck = REDIS_START_LOCK.lock().unwrap();

// The Redis server handling code is taken from the redis-rs project itself.
// https://github.com/mitsuhiko/redis-rs/blob/master/tests/test_basic.rs
// See LICENSE for a copy of the license
extern crate net2;
extern crate wait_timeout;

use std::process;
use std::thread::sleep;
use std::time::Duration;
use std::rc::Rc;
use self::wait_timeout::ChildExt;

pub struct RedisServer {
    pub process: process::Child,
    addr: redis::ConnectionAddr,
}

#[allow(unused)]
impl RedisServer {
    pub fn new() -> RedisServer {
        let mut retries_left = 3;

        loop {
            let mut cmd = process::Command::new("redis-server");
            // switch these to ::inherit() if you need to see redis output
            cmd.stdout(process::Stdio::null())
                .stderr(process::Stdio::null());

            // this is technically a race but we can't do better with
            // the tools that redis gives us :(
            let listener = net2::TcpBuilder::new_v4()
                .unwrap()
                .reuse_address(true)
                .unwrap()
                .bind("127.0.0.1:0")
                .unwrap()
                .listen(1)
                .unwrap();
            let server_port = listener.local_addr().unwrap().port();
            drop(listener);
            cmd.arg("--port")
                .arg(server_port.to_string())
                .arg("--bind")
                .arg("127.0.0.1");
            let addr = redis::ConnectionAddr::Tcp("127.0.0.1".to_string(), server_port);

            let mut process = cmd.spawn().unwrap();
            match process.wait_timeout(Duration::from_millis(500)).unwrap() {
                Some(err_status) => {
                    warn!("Redis child process exited unexpectedly early with exit status {}",
                          err_status);
                    if retries_left > 0 {
                        retries_left -= 1;
                        continue;
                    } else {
                        panic!("Failed to launch a redis sub-process that wouldn't exit \
                                immediately.");
                    }
                }
                None => {
                    // it's probably running fine
                }
            }
            return RedisServer { process: process, addr: addr };
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
        let mut server = RedisServer::new();

        let client = redis::Client::open(redis::ConnectionInfo {
                addr: Box::new(server.get_client_addr().clone()),
                db: 0,
                passwd: None,
            })
            .unwrap();
        let con;

        const MAX_WAIT_MS: u64 = 1000;
        const WAIT_INTERVAL_MS: u64 = 1;
        let mut waited_ms = 0;
        let wait_interval = Duration::from_millis(WAIT_INTERVAL_MS);
        loop {
            match client.get_connection() {
                Err(err) => {
                    if err.is_connection_refusal() {
                        if waited_ms < MAX_WAIT_MS {
                            waited_ms += 1;
                            sleep(wait_interval);
                        } else {
                            let exit = server.process.wait().unwrap();
                            panic!("Could not connect to ad-hoc redis instance after {}ms. \
                                    Server address: {:?}, error: {}. Redis process status: {:?}",
                                   MAX_WAIT_MS,
                                   server.addr,
                                   err,
                                   exit.code());
                        }
                    } else {
                        panic!("Could not connect to ad-hoc redis instance: {}", err);
                    }
                }
                Ok(x) => {
                    con = x;
                    break;
                }
            }
        }
        redis::cmd("FLUSHDB").execute(&con);

        TestContext { server: server, client: client }
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

use publisher::Publication;
use domain_spec::DomainSpec;
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

    // #### WHEN  ####
    redis_publisher.publish(&Publication { host: "example.com".to_owned(), specs: Vec::new() })
        .unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 0, "zero domain specs expected, got {:?}", specs);
}

#[test]
fn test_domains() {
    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.com".to_owned(),
                        http_port: Some(81),
                        https_port: Some(444),
                    },
                    DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: None,
                        https_port: Some(8443),
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 2, "two domain specs expected, got {:#?}", specs);

    for rawspec in specs {
        let (id, spec) = parse_domain_config(rawspec, "example.com");
        let expected = if id.contains("www") {
            &publication.specs[0]
        } else {
            &publication.specs[1]
        };
        assert_eq!(spec.domain_name, expected.domain_name);
        assert_eq!(spec.http_port, expected.http_port);
        assert_eq!(spec.https_port, expected.https_port);
    }
}

#[test]
fn test_domains_update() {
    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let mut publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.com".to_owned(),
                        http_port: Some(81),
                        https_port: Some(444),
                    },
                    DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: None,
                        https_port: Some(8443),
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();
    // then change the config and publish again
    publication.specs[0].http_port = Some(82);
    publication.specs[1].domain_name = "admin2.example.com".to_owned();
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    // Afterwards, we should see the new configuration.
    // Nothing should be left of the first version.
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 2, "two domain specs expected, got {:#?}", specs);

    for rawspec in specs {
        let (id, spec) = parse_domain_config(rawspec, "example.com");
        let expected = if id.contains("www") {
            &publication.specs[0]
        } else {
            &publication.specs[1]
        };
        assert_eq!(spec.domain_name, expected.domain_name);
        assert_eq!(spec.http_port, expected.http_port);
        assert_eq!(spec.https_port, expected.https_port);
    }
}

#[test]
fn test_domains_multi_host() {
    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let other_publication = Publication {
        host: "example.org".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.org".to_owned(),
                        http_port: Some(83),
                        https_port: Some(446),
                    },
                    DomainSpec {
                        domain_name: "admin.example.org".to_owned(),
                        http_port: None,
                        https_port: Some(8448),
                    }],
    };
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.com".to_owned(),
                        http_port: Some(81),
                        https_port: Some(444),
                    },
                    DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: None,
                        https_port: Some(8443),
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&other_publication).unwrap();
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 2, "Expected Redis to contain exactly 2 keys. Actual: {:?}", keys);

    let effective_key = keys.into_iter().filter(|x| x.contains("example.com")).next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 2, "two domain specs expected, got {:#?}", specs);

    for rawspec in specs {
        let (id, spec) = parse_domain_config(rawspec, "example.com");
        let expected = if id.contains("www") {
            &publication.specs[0]
        } else {
            &publication.specs[1]
        };
        assert_eq!(spec.domain_name, expected.domain_name);
        assert_eq!(spec.http_port, expected.http_port);
        assert_eq!(spec.https_port, expected.https_port);
    }
}

#[test]
fn test_domains_multi_host2() {
    // same as test_domains_multi_host, but the order in which the two hosts are being published
    // are reversed.
    // This makes sure that one doesn't overwrite the other.

    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let other_publication = Publication {
        host: "example.org".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.org".to_owned(),
                        http_port: Some(83),
                        https_port: Some(446),
                    },
                    DomainSpec {
                        domain_name: "admin.example.org".to_owned(),
                        http_port: None,
                        https_port: Some(8448),
                    }],
    };
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "www.example.com".to_owned(),
                        http_port: Some(81),
                        https_port: Some(444),
                    },
                    DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: None,
                        https_port: Some(8443),
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();
    redis_publisher.publish(&other_publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 2, "Expected Redis to contain exactly 2 keys. Actual: {:?}", keys);

    let effective_key = keys.into_iter().filter(|x| x.contains("example.com")).next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() == 2, "two domain specs expected, got {:#?}", specs);

    for rawspec in specs {
        let (id, spec) = parse_domain_config(rawspec, "example.com");
        let expected = if id.contains("www") {
            &publication.specs[0]
        } else {
            &publication.specs[1]
        };
        assert_eq!(spec.domain_name, expected.domain_name);
        assert_eq!(spec.http_port, expected.http_port);
        assert_eq!(spec.https_port, expected.https_port);
    }
}


#[test]
fn test_domain_id() {
    // This test verifies that the ID field contains a valid identifier.
    // More in-depth testing of the ID conversion is done in unit tests. Here, we just check
    // whether that code is actually applied to the contents of the ID field.

    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "admin-2.ex-ample.com".to_owned(),
                        http_port: Some(80),
                        https_port: None,
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rawpub: Json = tc.client.get(effective_key).unwrap();

    assert!(rawpub.is_array(),
            "top-level value stored in redis must be an array, was {:?}",
            rawpub);
    let specs: &json::Array = rawpub.as_array().unwrap();
    assert!(specs.len() > 0, "at least one domain spec expected, got {:#?}", specs);

    for rawspec in specs {
        let (id, _) = parse_domain_config(rawspec, "example.com");
        assert!(!id.contains("."), "The domain id must not contain dots: {}", id);
        assert!(!id.contains("-"), "The domain id must not contain dashes: {}", id);
    }
}

#[test]
fn test_ttl_no_expire() {
    // This test verifies that the ID field contains a valid identifier.
    // More in-depth testing of the ID conversion is done in unit tests. Here, we just check
    // whether that code is actually applied to the contents of the ID field.

    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    config.expire_seconds = None;
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: Some(80),
                        https_port: None,
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rcon = tc.client.get_connection().unwrap();
    let ttl: i64 = redis::cmd("TTL").arg(effective_key).query(&rcon).unwrap();
    // -1: no expiration (but key definitely exists)
    // -2: expired (really: "key does not exist")

    assert_eq!(ttl, -1);
}

#[test]
fn test_ttl_expire() {
    // This test verifies that the ID field contains a valid identifier.
    // More in-depth testing of the ID conversion is done in unit tests. Here, we just check
    // whether that code is actually applied to the contents of the ID field.

    common::init_log();
    // #### GIVEN ####
    let tc = TestContext::new();
    let mut config = Config::default();
    config.expire_seconds = Some(5);
    tc.server.configure(&mut config);
    let config = Arc::new(config);
    let mut redis_publisher = ::publisher::redis::RedisPublisher::new(config.clone());
    let publication = Publication {
        host: "example.com".to_owned(),
        specs: vec![DomainSpec {
                        domain_name: "admin.example.com".to_owned(),
                        http_port: Some(80),
                        https_port: None,
                    }],
    };

    // #### WHEN  ####
    redis_publisher.publish(&publication).unwrap();

    // #### THEN  ####
    let mut key_query = (*config.key_prefix).to_owned();
    key_query.push_str("*");
    let keys: Vec<String> = tc.client.keys(key_query).unwrap();
    assert!(keys.len() == 1, "Expected Redis to contain exactly 1 key. Actual: {:?}", keys);

    let effective_key = keys.into_iter().next().unwrap();
    let rcon = tc.client.get_connection().unwrap();
    let ttl: i64 = redis::cmd("TTL").arg(effective_key).query(&rcon).unwrap();
    // -1: no expiration (but key definitely exists)
    // -2: expired (really: "key does not exist")

    // The program should be fast enough so that this counter hasn't been decremented
    let expected_ttl = config.expire_seconds.unwrap() as i64;
    assert!(ttl == expected_ttl || ttl == expected_ttl - 1,
            "TTL of the config entry is expected to be close to {}. Was: {}",
            expected_ttl,
            ttl);
}

fn parse_domain_config(raw_domain_config: &Json, expected_host: &str) -> (String, DomainSpec) {
    use publisher::json_serializer::*;

    assert!(raw_domain_config.is_object(),
            "Expected domain configuration element to be a JSON object. Was {:?}",
            raw_domain_config);
    let raw_domain_config = raw_domain_config.as_object().unwrap();

    // ID
    let id = raw_domain_config.get(JSON_ID);
    assert!(id.is_some(), "expected {} field in {:#?}", JSON_ID, raw_domain_config);
    let id = id.unwrap();
    assert!(id.is_string(),
            "expected {} field to be a string in {:#?}",
            JSON_ID,
            raw_domain_config);
    let id = id.as_string().unwrap().to_owned();

    // DOMAIN
    let domain = raw_domain_config.get(JSON_DOMAIN);
    assert!(domain.is_some(), "expected {} field in {:#?}", JSON_DOMAIN, raw_domain_config);
    let domain = domain.unwrap();
    assert!(domain.is_string(),
            "expected {} field to be a string. Parent: {:#?}",
            JSON_DOMAIN,
            raw_domain_config);
    let domain = domain.as_string().unwrap().to_owned();

    // HTTP
    let http = parse_host_config(raw_domain_config, JSON_HTTP, expected_host);
    let https = parse_host_config(raw_domain_config, JSON_HTTPS, expected_host);

    (id, DomainSpec { domain_name: domain, http_port: http, https_port: https })
}

fn parse_host_config(raw_domain_config: &json::Object,
                     protocol_key: &str,
                     expected_host: &str)
                     -> Option<u16> {
    use publisher::json_serializer::*;
    use ::std;
    if let Some(protocol_config) = raw_domain_config.get(protocol_key) {
        assert!(protocol_config.is_object(),
                "Expected {} field to be an object. Parent: {:#?}",
                protocol_key,
                raw_domain_config);
        let protocol_config: &json::Object = protocol_config.as_object().unwrap();

        // HOST
        let host = protocol_config.get(JSON_HOST);
        assert!(host.is_some(),
                "Expected {} field in {} config: {:#?}.",
                JSON_HOST,
                protocol_key,
                raw_domain_config);
        let host = host.unwrap();
        assert!(host.is_string(),
                "Expected {} field in {} config to be a string. Was: {:?}",
                JSON_HOST,
                protocol_key,
                host);
        let host = host.as_string().unwrap();
        assert_eq!(host, expected_host);

        // PORT
        let port = protocol_config.get(JSON_PORT);
        assert!(port.is_some(),
                "Expected {} field in {} config: {:#?}",
                JSON_PORT,
                protocol_key,
                raw_domain_config);
        let port = port.unwrap();
        // is_i64 is too exact. Just check if it is convertible
        assert!(port.as_i64().is_some(),
                "Expected {} field in {} config to be a string. Was: {:?}",
                JSON_PORT,
                protocol_key,
                port);
        let port = port.as_i64().unwrap();
        assert!(0 < port && port <= std::u16::MAX as i64,
                "Port number must be a 16bit unsigned integer. Was {}",
                port);
        Some(port as u16)
    } else {
        None
    }
}
