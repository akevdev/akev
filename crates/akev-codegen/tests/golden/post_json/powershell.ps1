$headers = @{
    'Accept' = 'application/json'
}
$body = '{"name":"Ann O''Neil","age":34,"admin":false,"tags":["a","b"],"address":{"city":"Zürich","zip":null},"score":9.5,"meta":{}}'

$response = Invoke-WebRequest -Uri 'https://api.example.com/v1/users' `
    -Method Post `
    -Headers $headers `
    -ContentType 'application/json' `
    -Body ([System.Text.Encoding]::UTF8.GetBytes($body))

$response.StatusCode
$response.Content