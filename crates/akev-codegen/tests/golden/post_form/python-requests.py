import requests

url = "https://example.com/login"
headers = {
    "Content-Type": "application/x-www-form-urlencoded",
    "Cookie": "session=abc123; theme=dark",
    "Origin": "https://example.com",
}
payload = {
    "user": "ann@example.com",
    "password": "p@ss w0rd!",
    "remember": "on",
}

response = requests.post(url, headers=headers, data=payload)

print(response.status_code)
print(response.text)