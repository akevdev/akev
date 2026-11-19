// Cargo.toml:
// [dependencies]
// reqwest = "0.13"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = "user=ann%40example.com&password=p%40ss+w0rd%21&remember=on";
    let client = reqwest::Client::new();
    let response = client
        .post("https://example.com/login")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Cookie", "session=abc123; theme=dark")
        .header("Origin", "https://example.com")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}