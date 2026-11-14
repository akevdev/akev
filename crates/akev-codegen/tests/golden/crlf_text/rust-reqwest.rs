// Cargo.toml:
// [dependencies]
// reqwest = "0.12"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = "--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi \u{1B}[1m\"there\"\r\n--XyZ--\r\n";
    let client = reqwest::Client::new();
    let response = client
        .post("https://example.com/upload")
        .header("Content-Type", "multipart/form-data; boundary=XyZ")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}