use std::fmt::{self, Display};
use std::error::Error;
use std::num::ParseIntError;

use regex::Regex;

use common::optional_result;

/// Specification for a single domain.
/// Contains either an http, an https or both ports.
#[derive(Debug)]
pub struct DomainSpec {
    pub domain_name: String,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
}

lazy_static! {
    static ref DS_PAT: Regex =
        Regex::new(r"([a-zA-Z0-9][a-zA-Z0-9.-]+[a-zA-Z0-9]\.?)(:(\S+))?").unwrap();
    static ref ID_PAT: Regex = Regex::new(r"[^A-Za-z0-9_]").unwrap();
}

impl DomainSpec {
    pub fn spec_id(&self) -> String {
        ID_PAT.replace_all(&self.domain_name, "_")
    }

    pub fn parse_all(raw: &str, specs: &mut Vec<DomainSpec>) -> Result<(),DomainSpecError> {
        fn parse_port(key: &str, value: Option<&str>, spec: &DomainSpec) -> Result<Option<u16>, DomainSpecError> {
            match optional_result(value.map(|v| u16::from_str_radix(v,10))) {
                Err(e) => Err(DomainSpecError { domain_name: spec.domain_name.clone(), cause: e, key: Some(key.to_owned()) }),
                Ok(port) => Ok(port)
            }
        }
        for captures in DS_PAT.captures_iter(raw) {
            // The first capture group is guaranteed to be there.
            let mut domain_name = captures.at(1).unwrap();

            // Strip . at the end of FQDN
            if domain_name.ends_with('.') {
                // Chop off '.' at the end
                domain_name = &domain_name[0..(domain_name.len() - 1)]
            }

            let raw_params = captures.at(3)
            .map(|params| params.trim().split(':').collect())
            .unwrap_or_else(||Vec::new());

            let mut spec = DomainSpec {
                domain_name: domain_name.to_owned(),
                http_port: None,
                https_port: None
            };
            for raw_param in raw_params {
                let param_parts : Vec<&str> = raw_param.splitn(2,'=').collect();

                let key = param_parts[0].trim().to_lowercase();
                let value = if param_parts.len() > 1 {
                    Some(param_parts[1].trim())
                } else {
                    None
                };

                // Merely having an 'http' or 'https' key present, enables the mapping
                match key.as_str() {
                    "http" => {
                        let parsed_port_opt = try!(parse_port(&key, value, &spec));
                        spec.http_port = Some(parsed_port_opt.unwrap_or(80));
                    },
                    "https" => {
                        let parsed_port_opt = try!(parse_port(&key, value, &spec));
                        spec.https_port = Some(parsed_port_opt.unwrap_or(443));
                    },
                    _ => {
                        warn!("Unknown domain spec parameter ");
                    }
                }
            }

            // If neither http nor https have been specified, assume both mappings.
            if spec.http_port.is_none() && spec.https_port.is_none() {
                spec.http_port = Some(80);
                spec.https_port = Some(443);
            }

            specs.push(spec);
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct DomainSpecError {
    pub domain_name: String,

    // move into enum when more options come
    pub cause: ParseIntError,
    pub key: Option<String>
}

impl Display for DomainSpecError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(fmt, "{} Domain name: \"{}\"", self.description(), self.domain_name));
        if let Some(ref key) = self.key {
            try!(write!(fmt, " Option name: \"{}\"", key));
        }
        write!(fmt, " Cause: {}", self.cause)
    }
}

impl Error for DomainSpecError {
    fn description(&self) -> &str {
        "Failed to parse domain spec option."
    }
    fn cause(&self) -> Option<&Error> {
        Some(&self.cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common;
    use regex::Regex;

    fn assert_valid_id(spec: &DomainSpec) {
        lazy_static! {
            static ref VALID_ID_PAT : Regex = Regex::new(r"^[A-Za-z0-9_]+$").unwrap();
        }
        let id = spec.spec_id();
        assert!(VALID_ID_PAT.is_match(&id),
            "The id \"{}\" derived from domain {} is contains invalid characters.",
            id, spec.domain_name);
    }

    #[test]
    fn empty_spec_valid() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("", &mut specs).expect("Parse \"\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 0);
    }

    #[test]
    fn implicit_http_https() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org", &mut specs)
            .expect("Parse \"example.org\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "example.org");
        assert_eq!(spec.http_port, Some(80));
        assert_eq!(spec.https_port, Some(443));
        assert_valid_id(spec);
    }

    #[test]
    fn chop_off_fqdn_dot() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org.", &mut specs)
        .expect("Parse \"example.org.\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "example.org");
        assert_eq!(spec.http_port, Some(80));
        assert_eq!(spec.https_port, Some(443));
        assert_valid_id(spec);
    }

    #[test]
    fn explicit_protocol_implicit_port_http() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org:http", &mut specs)
        .expect("Parse \"example.org:http\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "example.org");
        assert_eq!(spec.http_port, Some(80));
        assert_eq!(spec.https_port, None);
        assert_valid_id(spec);
    }

    #[test]
    fn explicit_protocol_implicit_port_https() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org:https", &mut specs)
        .expect("Parse \"example.org:https\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "example.org");
        assert_eq!(spec.http_port, None);
        assert_eq!(spec.https_port, Some(443));
        assert_valid_id(spec);
    }

    #[test]
    fn fully_explicit() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org:http=8080:https=8043", &mut specs)
        .expect("Parse \"example.org:http=8080:https=8043\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "example.org");
        assert_eq!(spec.http_port, Some(8080));
        assert_eq!(spec.https_port, Some(8043));
        assert_valid_id(spec);
    }

    #[test]
    fn complex() {
        common::init_log();
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("example.org:http:https=8043 admin.example.org:https=9043 www.example.org", &mut specs)
        .expect("Parse \"example.org:http:https=8043 admin.example.org:https=9043 www.example.org\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 3);
        let spec1 = &specs[0];
        assert_eq!(spec1.domain_name, "example.org");
        assert_eq!(spec1.http_port, Some(80));
        assert_eq!(spec1.https_port, Some(8043));
        assert_valid_id(spec1);

        let spec2 = &specs[1];
        assert_eq!(spec2.domain_name, "admin.example.org");
        assert_eq!(spec2.http_port, None);
        assert_eq!(spec2.https_port, Some(9043));
        assert_valid_id(spec2);

        let spec3 = &specs[2];
        assert_eq!(spec3.domain_name, "www.example.org");
        assert_eq!(spec3.http_port, Some(80));
        assert_eq!(spec3.https_port, Some(443));
        assert_valid_id(spec3);
    }

    #[test]
    fn spec_id_dash_dot() {
        // #### GIVEN ####
        let mut specs = Vec::new();

        // #### WHEN  ####
        DomainSpec::parse_all("admin-internal.example.org:http=8080:https=8043", &mut specs)
        .expect("Parse \"admin-internal.example.org:http=8080:https=8043\" successfully");

        // #### THEN  ####
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.domain_name, "admin-internal.example.org");
        assert_eq!(spec.http_port, Some(8080));
        assert_eq!(spec.https_port, Some(8043));
        assert_valid_id(spec);
    }
}