use std::collections::HashMap;

use tiny_http::{Header, Method, Request, Response, Server};

/// Demo-only: serve a fixed set of static routes over HTTP until a fatal error.
///
/// `paths[i]` is answered with `bodies[i]` (HTTP 200, `text/plain`); any other
/// path returns 404, and a non-GET request to a known path returns 405. `paths`
/// and `bodies` must have equal length, and paths must be unique. A successful
/// bind serves forever, so this only returns on a bind/setup error.
pub fn serve_static(address: &str, paths: &[String], bodies: &[String]) -> Result<(), String> {
    let routes = build_routes(paths, bodies)?;
    let server = Server::http(address).map_err(|error| error.to_string())?;
    for request in server.incoming_requests() {
        respond(request, &routes)?;
    }
    Ok(())
}

/// Pair `paths` with `bodies` into a route table, rejecting mismatched lengths
/// and duplicate paths so the served routes are unambiguous.
fn build_routes(paths: &[String], bodies: &[String]) -> Result<HashMap<String, String>, String> {
    if paths.len() != bodies.len() {
        return Err(format!(
            "http-server: paths ({}) and bodies ({}) must have the same length.",
            paths.len(),
            bodies.len()
        ));
    }
    let mut routes = HashMap::with_capacity(paths.len());
    for (path, body) in paths.iter().zip(bodies.iter()) {
        if routes.insert(path.clone(), body.clone()).is_some() {
            return Err(format!("http-server: duplicate route `{path}`."));
        }
    }
    Ok(routes)
}

fn respond(request: Request, routes: &HashMap<String, String>) -> Result<(), String> {
    let is_get = request.method() == &Method::Get;
    let (status, body) = resolve_route(routes, request.url(), is_get);
    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(text_plain_header());
    request
        .respond(response)
        .map_err(|error| format!("http-server: failed to send response: {error}"))
}

/// Decide the `(status, body)` for a request. Split out from socket handling so
/// the routing rules can be unit-tested directly.
fn resolve_route(routes: &HashMap<String, String>, url: &str, is_get: bool) -> (u16, String) {
    let path = url.split('?').next().unwrap_or(url);
    match routes.get(path) {
        Some(_) if !is_get => (405, "Method Not Allowed".to_string()),
        Some(body) => (200, body.clone()),
        None => (404, "Not Found".to_string()),
    }
}

fn text_plain_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..])
        .expect("static content-type header is valid")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::thread;

    use super::*;

    fn routes(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string()))
            .collect()
    }

    #[test]
    fn build_routes_rejects_length_mismatch() {
        let error = build_routes(&["/a".to_string()], &[]).unwrap_err();
        assert!(error.contains("same length"), "got: {error}");
    }

    #[test]
    fn build_routes_rejects_duplicate_paths() {
        let paths = vec!["/a".to_string(), "/a".to_string()];
        let bodies = vec!["x".to_string(), "y".to_string()];
        let error = build_routes(&paths, &bodies).unwrap_err();
        assert!(error.contains("duplicate route"), "got: {error}");
    }

    #[test]
    fn resolve_route_serves_known_get_and_ignores_query() {
        let table = routes(&[("/hello", "world")]);
        assert_eq!(
            resolve_route(&table, "/hello", true),
            (200, "world".to_string())
        );
        assert_eq!(
            resolve_route(&table, "/hello?name=x", true),
            (200, "world".to_string())
        );
    }

    #[test]
    fn resolve_route_returns_404_and_405() {
        let table = routes(&[("/hello", "world")]);
        assert_eq!(resolve_route(&table, "/missing", true).0, 404);
        assert_eq!(resolve_route(&table, "/hello", false).0, 405);
    }

    #[test]
    fn serves_a_real_request_over_a_socket() {
        let table = routes(&[("/hello", "world")]);
        let server = Server::http("127.0.0.1:0").expect("ephemeral bind should succeed");
        let addr = server
            .server_addr()
            .to_ip()
            .expect("ephemeral listener has an ip address");

        thread::spawn(move || {
            // Serve exactly the one request this test issues, then stop.
            if let Ok(request) = server.recv() {
                respond(request, &table).expect("test response should send");
            }
        });

        let mut stream = TcpStream::connect(addr).expect("client should connect");
        stream
            .write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("client should send request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("client should read response");

        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert!(response.contains("world"), "got: {response}");
    }
}
