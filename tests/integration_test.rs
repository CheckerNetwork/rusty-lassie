use pretty_assertions::assert_eq;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use lassie::{Daemon, DaemonConfig};

// Rust runs tests in parallel. Since Lassie Daemon is a singleton,
// we must synchronise the tests to ensure they run sequentially
static TEST_GUARD: Mutex<()> = Mutex::new(());

#[test]
fn start_daemon_and_request_cid() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig::default()).expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafkreih25dih6ug3xtj73vswccw423b56ilrwmnos4cbwhrceudopdp5sq?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url)
        .set("Accept", "application/vnd.ipld.car")
        .call();
    let response = assert_ok_response(response);

    println!("response headers: {:?}", response.headers_names());
    for hn in &response.headers_names() {
        println!("\t{hn}: {}", response.header(hn).unwrap_or("<empty>"));
    }

    assert_eq!(
        response.header("Content-Type"),
        Some("application/vnd.ipld.car;version=1;order=dfs;dups=y")
    );

    let mut content = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut content)
        .expect("cannot read response body");

    assert_eq!(
        content,
        include_bytes!("testdata/bafkreih25dih6ug3xtj73vswccw423b56ilrwmnos4cbwhrceudopdp5sq.car")
    );
}

#[test]
fn configure_max_blocks() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig {
        max_blocks: Some(1),
        ..DaemonConfig::default()
    })
    .expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    // This archive contains many blocks and takes long to download unless the block limit is applied
    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafybeih5zasorm4tlfga4ztwvm2dlnw6jxwwuvgnokyt3mjamfn3svvpyy?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url).call();
    let response = assert_ok_response(response);

    let mut content = Vec::new();
    let error = response
        .into_reader()
        .read_to_end(&mut content)
        .expect_err("response stream should have been aborted by the server");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn configure_global_timeout() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig {
        global_timeout: Some(Duration::from_millis(1000)),
        ..DaemonConfig::default()
    })
    .expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    // This archive contains many blocks and takes long to download unless the block limit is applied
    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafybeih5zasorm4tlfga4ztwvm2dlnw6jxwwuvgnokyt3mjamfn3svvpyy?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url).call();
    let response = assert_ok_response(response);

    let mut content = Vec::new();
    let error = response
        .into_reader()
        .read_to_end(&mut content)
        .expect_err("response stream should have been aborted by the server");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn it_rejects_anonymous_requests_when_configured_with_access_token() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig {
        access_token: Some("super_secret".to_string()),
        ..DaemonConfig::default()
    })
    .expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafkreih25dih6ug3xtj73vswccw423b56ilrwmnos4cbwhrceudopdp5sq?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url)
        .set("Accept", "application/vnd.ipld.car")
        .call();

    assert_response_error(response, 401);
}

#[test]
fn it_allows_authorized_requests_when_configured_with_access_token() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig {
        access_token: Some("super_secret".to_string()),
        ..DaemonConfig::default()
    })
    .expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafkreih25dih6ug3xtj73vswccw423b56ilrwmnos4cbwhrceudopdp5sq?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url)
        .set("Accept", "application/vnd.ipld.car")
        .set(
            "Authorization",
            &format!("Bearer {}", daemon.access_token().as_ref().unwrap()),
        )
        .call();
    assert_ok_response(response);
}

#[test]
fn it_rejects_incorrect_authorization_when_configured_with_access_token() {
    let _lock = setup_test_env();

    let daemon = Daemon::start(DaemonConfig {
        access_token: Some("super_secret".to_string()),
        ..DaemonConfig::default()
    })
    .expect("cannot start Lassie");
    let port = daemon.port();
    assert!(port > 0, "Lassie is listening on non-zero port number");

    let url = format!(
        "http://127.0.0.1:{port}/ipfs/bafkreih25dih6ug3xtj73vswccw423b56ilrwmnos4cbwhrceudopdp5sq?protocol=http&providers=/dns4/frisbii.fly.dev/https"
    );
    let response = ureq::get(&url)
        .set("Accept", "application/vnd.ipld.car")
        .set("Authorization", "Bearer wrong-token")
        .call();

    assert_response_error(response, 401);
}

fn setup_test_env() -> MutexGuard<'static, ()> {
    let _ = env_logger::builder().is_test(true).try_init();
    let lock = TEST_GUARD.lock().expect("cannot obtain global test lock. This typically happens when one of the test fails; the problem should go away after you fix the test failure.");
    lock
}

fn assert_ok_response(response: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    if let Err(ureq::Error::Status(code, response)) = response {
        panic!(
            "Request failed with status {}. Body:\n{}\n==EOF==",
            code,
            response.into_string().expect("cannot read response body")
        );
    }

    let response = response.expect("cannot fetch CID using Lassie");
    assert_eq!(response.status(), 200);

    response
}

fn assert_response_error(response: Result<ureq::Response, ureq::Error>, expected_code: u16) {
    match response {
        Err(ureq::Error::Status(code, response)) => {
            assert!(
                code == expected_code,
                "Request failed with unexpected status code. Wanted: {expected_code} Found: {code}. Body:\n{}\n==EOF==",
                response.into_string().expect("cannot read response body"),
            );
        }

        Err(err) => {
            panic!("Request failed with unexpected error: {err:?}");
        }

        Ok(_response) => {
            panic!("Request should have failed with {expected_code}, it succeeded instead.");
        }
    }
}
