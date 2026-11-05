use craftsman_gadget::{GadgetError, JsonSchema, gadget_call, host};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, JsonSchema)]
struct Echo {
    text: String,
}

/// Echo a string through the standardized entry point.
#[gadget_call]
fn echo(args: Echo) -> Result<Value, GadgetError> {
    Ok(json!({"text": args.text, "version": env!("CARGO_PKG_VERSION")}))
}

/// Return a pure value without arguments.
#[gadget_call]
fn ping() -> Result<Value, GadgetError> {
    Ok(json!({"pong": true}))
}

#[derive(Deserialize, JsonSchema)]
struct File {
    path: String,
}

/// Read a file through the host capability check.
#[gadget_call]
fn read(args: File) -> Result<Value, GadgetError> {
    let bytes = host::read_file(&args.path)?;
    Ok(json!({"text": String::from_utf8_lossy(&bytes)}))
}

/// List a directory through the host capability check.
#[gadget_call]
fn list(args: File) -> Result<Value, GadgetError> {
    Ok(serde_json::to_value(host::list_dir(&args.path)?).unwrap())
}

#[derive(Deserialize, JsonSchema)]
struct Write {
    path: String,
    text: String,
}

/// Write a file through the host capability check and approval gate.
#[gadget_call]
fn write(args: Write) -> Result<Value, GadgetError> {
    host::write_file(&args.path, args.text.as_bytes())?;
    Ok(json!({"written": true}))
}

#[derive(Deserialize, JsonSchema)]
struct Url {
    url: String,
}

/// Request HTTPS through the host's allowlist.
#[gadget_call]
fn http(args: Url) -> Result<Value, GadgetError> {
    let result = host::http_request(host::Request {
        method: "GET".into(),
        url: args.url,
        headers: vec![],
        body: None,
    })?;
    Ok(json!({"status": result.status}))
}

#[derive(Deserialize, JsonSchema)]
struct Probe {
    #[serde(default = "default_address")]
    address: String,
}
fn default_address() -> String {
    "127.0.0.1:9".into()
}

/// Probe unavailable ambient WASI capabilities.
#[gadget_call]
fn ambient(args: Probe) -> Result<Value, GadgetError> {
    Ok(json!({
        "env": std::env::var("GADGET_TEST_SECRET").ok(),
        "file": std::fs::read_to_string("Cargo.toml").is_ok(),
        "socket": std::net::TcpStream::connect(&args.address).is_ok(),
    }))
}

/// Run forever; the host must interrupt this call.
#[gadget_call]
fn spin() -> Result<(), GadgetError> {
    loop {
        std::hint::spin_loop();
    }
}

/// Request a gigabyte allocation; the host must bound the linear memory.
#[gadget_call]
fn allocate() -> Result<u64, GadgetError> {
    let bytes = std::hint::black_box(vec![1u8; 1024 * 1024 * 1024]);
    Ok(bytes.iter().map(|b| *b as u64).sum())
}

/// Return an oversized JSON result.
#[gadget_call]
fn enormous() -> Result<String, GadgetError> {
    Ok("x".repeat(10 * 1024 * 1024))
}

/// Flood stdout, stderr and the host log without touching the terminal.
#[gadget_call]
fn output() -> Result<Value, GadgetError> {
    for _ in 0..1000 {
        println!("{}", "x".repeat(100));
        eprintln!("{}", "y".repeat(100));
        host::log(host::Level::Info, &"z".repeat(100));
    }
    Ok(json!({"done": true}))
}

/// Sleep in a WASI host call; the outer wall-clock timeout must still apply.
#[gadget_call]
fn sleep() -> Result<(), GadgetError> {
    std::thread::sleep(std::time::Duration::from_secs(3600));
    Ok(())
}
