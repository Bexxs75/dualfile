//! Data connection addresses announced by the server (`PASV`/`EPSV`) and the bounce
//! protection: the data connection always goes to the host of the control connection; an
//! announced foreign address is dropped, and ports that belong to other services are refused.

use crate::error::{TResult, TransportError};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Lowest port a data connection may use (below it live the well-known services a hostile
/// server could point the client at).
pub const MIN_DATA_PORT: u16 = 1024;

fn protocol(text: &'static str) -> TransportError {
    TransportError::Protocol(text.to_owned())
}

/// `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)` → address and port.
pub fn parse_pasv(reply: &str) -> TResult<(Ipv4Addr, u16)> {
    let open = reply
        .find('(')
        .ok_or_else(|| protocol("PASV-Antwort ohne Adresse"))?;
    let close = reply[open..]
        .find(')')
        .ok_or_else(|| protocol("PASV-Antwort ohne Adresse"))?
        + open;
    let numbers: Vec<u8> = reply[open + 1..close]
        .split(',')
        .map(|part| part.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .map_err(|_| protocol("PASV-Antwort ungültig"))?;
    if numbers.len() != 6 {
        return Err(protocol("PASV-Antwort ungültig"));
    }
    let ip = Ipv4Addr::new(numbers[0], numbers[1], numbers[2], numbers[3]);
    let port = (u16::from(numbers[4]) << 8) | u16::from(numbers[5]);
    Ok((ip, port))
}

/// `229 Entering Extended Passive Mode (|||port|)` → port. The delimiter may be any
/// printable character other than a digit, but must be the same four times.
pub fn parse_epsv(reply: &str) -> TResult<u16> {
    let bad = || protocol("EPSV-Antwort ungültig");
    let open = reply.find('(').ok_or_else(bad)?;
    let close = reply[open..].find(')').ok_or_else(bad)? + open;
    let inner = &reply[open + 1..close];
    let mut chars = inner.chars();
    let delimiter = chars.next().ok_or_else(bad)?;
    if delimiter.is_ascii_digit() || delimiter.is_control() || delimiter == ' ' {
        return Err(bad());
    }
    let fields: Vec<&str> = inner.split(delimiter).collect();
    // `|||6446|` splits into ["", "", "", "6446", ""]
    if fields.len() != 5
        || !fields[0].is_empty()
        || !fields[1].is_empty()
        || !fields[2].is_empty()
        || !fields[4].is_empty()
    {
        return Err(bad());
    }
    fields[3].parse::<u16>().map_err(|_| bad())
}

/// The address a data connection is opened to, given the control connection's peer address
/// and what the server announced. A foreign IP is replaced by the control peer's; a port
/// below `MIN_DATA_PORT` (including 0) is an error.
pub fn resolve_data_addr(control: IpAddr, announced: SocketAddr) -> TResult<SocketAddr> {
    if announced.port() < MIN_DATA_PORT {
        return Err(protocol("Der Server nennt einen unzulässigen Datenport"));
    }
    Ok(SocketAddr::new(control, announced.port()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pasv_and_epsv_replies() {
        let (ip, port) = parse_pasv("227 Entering Passive Mode (192,168,1,2,19,136).").unwrap();
        assert_eq!((ip, port), (Ipv4Addr::new(192, 168, 1, 2), 5000));
        assert_eq!(
            parse_epsv("229 Entering Extended Passive Mode (|||6446|)").unwrap(),
            6446
        );
        assert_eq!(parse_epsv("229 x (!!!2000!)").unwrap(), 2000);
        for bad in [
            "227 no address",
            "227 (1,2,3,4,5)",
            "227 (1,2,3,4,5,6,7)",
            "227 (256,2,3,4,5,6)",
            "227 (a,b,c,d,e,f)",
            "227 (1,2,3,4,5,6",
        ] {
            assert!(parse_pasv(bad).is_err(), "{bad}");
        }
        for bad in [
            "229 (|||65536|)",
            "229 (|||-1|)",
            "229 (||6446|)",
            "229 (|a||6446|)",
            "229 (123456)",
            "229 (1|||6446|)",
            "229 ()",
            "229 no",
            "229 (|||6446)",
        ] {
            assert!(parse_epsv(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_foreign_address_is_replaced_by_the_control_host() {
        let control: IpAddr = "203.0.113.5".parse().unwrap();
        let foreign: SocketAddr = "10.9.9.9:5000".parse().unwrap();
        assert_eq!(
            resolve_data_addr(control, foreign).unwrap(),
            "203.0.113.5:5000".parse().unwrap()
        );
        let same: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        assert_eq!(resolve_data_addr(control, same).unwrap(), same);
        // another service's port (SMTP, SSH, port 0) is never a data port
        for port in [0, 22, 25, 1023] {
            assert!(
                resolve_data_addr(control, SocketAddr::new(control, port)).is_err(),
                "{port}"
            );
        }
        assert!(resolve_data_addr(control, SocketAddr::new(control, 1024)).is_ok());
    }
}
