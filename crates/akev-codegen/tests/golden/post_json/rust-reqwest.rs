// Cargo.toml:
// [dependencies]
// reqwest = "0.12"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"{"name":"Ann O'Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}"#;
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.example.com/v1/users")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}