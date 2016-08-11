use rustc_serialize::json::{self, ToJson};

use domain_spec::DomainSpec;
use super::*;

pub const JSON_HOST: &'static str = "host";
pub const JSON_PORT: &'static str = "port";
pub const JSON_ID: &'static str = "id";
pub const JSON_DOMAIN: &'static str = "domain";
pub const JSON_HTTP: &'static str = "http";
pub const JSON_HTTPS: &'static str = "https";

pub fn svc_config<T: ToJson>(domain_config: &mut json::Object, field: &str, value_opt: Option<T>) {
    if let Some(value) = value_opt {
        domain_config.insert(field.to_owned(), value.to_json());
    }
}

pub fn backend_setup(host: &str, port: u16) -> Option<json::Object> {
    let mut setup = json::Object::new();
    setup.insert(JSON_HOST.to_owned(), host.to_owned().to_json());
    setup.insert(JSON_PORT.to_owned(), port.to_owned().to_json());
    Some(setup)
}

pub fn domain_config(container_host: &str, spec: &DomainSpec) -> json::Object {
    let mut domain_config = json::Object::new();
    svc_config(&mut domain_config, JSON_ID, Some(spec.spec_id()));
    svc_config(&mut domain_config, JSON_DOMAIN, Some(spec.domain_name.clone()));
    svc_config(&mut domain_config,
               JSON_HTTP,
               spec.http_port.map(|http_port| backend_setup(&container_host, http_port)));
    svc_config(&mut domain_config,
               JSON_HTTPS,
               spec.https_port.map(|https_port| backend_setup(&container_host, https_port)));
    domain_config
}

pub fn domain_configs(container_host: &str, specs: &[DomainSpec]) -> json::Array {
    let mut configs = json::Array::new();
    for spec in specs {
        let config = domain_config(container_host, spec);
        configs.push(config.to_json());
    }
    configs
}

// ############### PUBLISHING ERROR #######################
impl PublishingInnerError for json::EncoderError {}

// ############### TESTING ################################
#[cfg(test)]
mod tests {
    use super::*;
    use common;
    use domain_spec::DomainSpec;

    use rustc_serialize::json::{self, ToJson, Json, as_pretty_json};

    #[test]
    fn full_spec() {
        common::init_log();
        // #### GIVEN ####
        let host = "app-server";
        let spec = DomainSpec {
            domain_name: "example.org".to_owned(),
            http_port: Some(8080),
            https_port: Some(8043),
        };

        // #### WHEN  ####
        let cfg = domain_config(host, &spec).to_json();

        // #### THEN  ####
        assert_eq_domain_spec(&cfg, &host, &spec);
    }

    #[test]
    fn http_missing() {
        common::init_log();
        // #### GIVEN ####
        let host = "app-server";
        let spec = DomainSpec {
            domain_name: "example.org".to_owned(),
            http_port: None,
            https_port: Some(8043),
        };

        // #### WHEN  ####
        let cfg = domain_config(host, &spec).to_json();

        // #### THEN  ####
        assert_eq_domain_spec(&cfg, &host, &spec);
    }

    #[test]
    fn https_missing() {
        common::init_log();
        // #### GIVEN ####
        let host = "app-server";
        let spec = DomainSpec {
            domain_name: "example.org".to_owned(),
            http_port: Some(8080),
            https_port: None,
        };

        // #### WHEN  ####
        let cfg = domain_config(host, &spec).to_json();

        // #### THEN  ####
        assert_eq_domain_spec(&cfg, &host, &spec);
    }

    /// Standard ports shouldn't have special treatment.
    #[test]
    fn standard_ports() {
        common::init_log();
        // #### GIVEN ####
        let host = "app-server";
        let spec = DomainSpec {
            domain_name: "example.org".to_owned(),
            http_port: Some(80),
            https_port: Some(443),
        };

        // #### WHEN  ####
        let cfg = domain_config(host, &spec).to_json();

        // #### THEN  ####
        assert_eq_domain_spec(&cfg, &host, &spec);
    }

    #[test]
    fn multiple_configs() {
        common::init_log();
        // #### GIVEN ####
        let host = "app-server";
        let spec1 = DomainSpec {
            domain_name: "example.org".to_owned(),
            http_port: Some(80),
            https_port: Some(443),
        };
        let spec2 = DomainSpec {
            domain_name: "www.example.org".to_owned(),
            http_port: Some(8080),
            https_port: Some(8043),
        };

        // #### WHEN  ####
        let cfgs = domain_configs(host, &[spec1.clone(), spec2.clone()]).to_json();

        // #### THEN  ####
        // It isn't strictly necessary, that the specs come out in a particular order, but
        // I'm too lazy to write the assertion code for arbitrary orderings.
        match cfgs {
            Json::Array(ref cfgs) => {
                assert_eq_domain_spec(&cfgs[0], &host, &spec1);
                assert_eq_domain_spec(&cfgs[1], &host, &spec2);
            }
            other => {
                assert!(false, "Multiple domain configs, expected Json::Array, got {:?}", other)
            }
        }
    }

    fn assert_eq_domain_spec(val: &Json, host: &str, domain_spec: &DomainSpec) {
        fn assert_backend_spec(obj: &json::Object,
                               field: &str,
                               host: &str,
                               port_opt: Option<u16>) {
            match port_opt {
                Some(port) => {
                    assert_json_obj_field_present(&obj, field);
                    match obj.get(field).unwrap() {
                        &Json::Object(ref hp_obj) => {
                            assert_json_obj_field_eq(hp_obj, JSON_HOST, host);
                            assert_json_obj_field_eq(hp_obj, JSON_PORT, &port);
                        }
                        other => {
                            assert!(false,
                                    "For backend config, expected Json::Object, got {:?}",
                                    other)
                        }
                    }
                }
                None => {
                    assert_json_no_obj_field(obj, field);
                }
            }
        }
        let spec_id = domain_spec.spec_id();
        match val {
            &Json::Object(ref obj) => {
                assert_json_obj_field_eq(obj, JSON_ID, spec_id.as_str());
                assert_json_obj_field_eq(obj, JSON_DOMAIN, domain_spec.domain_name.as_str());
                assert_backend_spec(obj, JSON_HTTP, host, domain_spec.http_port);
                assert_backend_spec(obj, JSON_HTTPS, host, domain_spec.https_port);
            }
            other => assert!(false, "For domain config, expected Json::Object, got {:?}", other),
        }
    }

    fn assert_json_obj_field_eq<T: ToJson + ?Sized>(obj: &json::Object, field: &str, val: &T) {
        assert_json_obj_field_present(obj, field);
        let left_buf = format!("{}", as_pretty_json(&obj.get(field).unwrap()));
        let right_buf = format!("{}", as_pretty_json(&val.to_json()));
        assert_eq!(left_buf, right_buf);
    }

    fn assert_json_obj_field_present(obj: &json::Object, field: &str) {
        assert!(obj.contains_key(field),
                concat!("JSON object expected to have field {}. ", "No such field. Object: {:#?}"),
                field,
                obj);
    }

    fn assert_json_no_obj_field(obj: &json::Object, field: &str) {
        assert!(!obj.contains_key(field),
                concat!("JSON object NOT expected to have field {}. ",
                        "Field present, value: {:?}"),
                field,
                obj.get(field).unwrap());
    }
}
