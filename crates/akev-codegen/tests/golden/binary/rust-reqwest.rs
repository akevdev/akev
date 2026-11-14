// Cargo.toml:
// [dependencies]
// reqwest = "0.12"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body: &[u8] = b"\x89PNG\r\n\x1A\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1F\x15\xC4\x89\xFF\xFE\"'\\";
    let client = reqwest::Client::new();
    let response = client
        .put("https://upload.example.com/files/logo.png")
        .header("Content-Type", "image/png")
        .header("X-Upload-Token", "t0k3n")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}