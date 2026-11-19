// Cargo.toml:
// [dependencies]
// reqwest = "0.13"
// tokio = { version = "1", features = ["full"] }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"Line 1: it's "quoted" and 'single'
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\slash \n \\ """ ''' ``` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
'@ at line start, and a trailing backslash \"#;
    let client = reqwest::Client::new();
    let response = client
        .patch("https://example.com/api/notes/1?q=a%20b&x=$y&z='\"")
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("X-Quote", "it's \"quoted\" `cmd` $HOME %PATH% \\back\\")
        .header("X-Unicode", "héllo 日本 ✓")
        .header("X-Eq", "=starts-with-equals")
        .body(body)
        .send()
        .await?;

    println!("{}", response.status());
    println!("{}", response.text().await?);

    Ok(())
}