// The MIT License (MIT)
//
// Copyright (c) 2016 Christian Klauser
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::sync::Arc;

use rustc_serialize::json;
use redis as libredis;
use redis::{RedisResult, Commands};

use common::Config;
use super::*;
use super::json_serializer;

pub struct RedisPublisher {
    config: Arc<Config>,
    redis_client_opt: Option<libredis::Client>,
}

impl RedisPublisher {
    pub fn new(config: Arc<Config>) -> RedisPublisher {
        RedisPublisher { config: config, redis_client_opt: None }
    }

    fn create_redis_client(&mut self) -> RedisResult<&mut libredis::Client> {
        if let Some(ref mut client) = self.redis_client_opt {
            Ok(client)
        } else {
            let addr = libredis::ConnectionAddr::Tcp((*self.config.redis_host).clone(),
                                                     self.config.redis_port);
            let info = libredis::ConnectionInfo { addr: Box::new(addr), db: 0, passwd: None };
            let client = try!(libredis::Client::open(info));
            self.redis_client_opt = Some(client);
            Ok(self.redis_client_opt.as_mut().unwrap())
        }
    }
}

impl Publish for RedisPublisher {
    fn publish(&mut self, publication: &Publication) -> Result<(), PublishingError> {
        let config = self.config.clone();
        let r_client = try!(self.create_redis_client());

        let mut key = String::new();
        service_key(&config, &publication.host, &mut key);
        let key = key;

        let published_config = json_serializer::domain_configs(&publication.host,
                                                               &publication.specs);
        let redis_value = try!(json::encode(&published_config));


        if let Some(expire_seconds) = config.expire_seconds {
            try!(r_client.set_ex(key, redis_value, expire_seconds as usize));
        } else {
            try!(r_client.set(key, published_config));
        }

        Ok(())
    }
}

fn service_key(config: &Config, container_name: &str, key: &mut String) {
    key.push_str(&config.key_prefix);
    key.push_str(container_name);
}

// ############### PUBLISHING ERROR #######################
impl PublishingInnerError for libredis::RedisError {}

// ############### TESTING ################################
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::default::Default;

    use super::*;
    use common::{self, Config};

    #[test]
    fn new_redis() {
        common::init_log();
        // #### GIVEN ####
        let cfg = Arc::new(Config::default());

        // #### WHEN  ####
        RedisPublisher::new(cfg.clone());

        // #### THEN  ####
        // doesn't panic
    }
}
