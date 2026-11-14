import requests

url = "https://api.example.com/v1/users"
headers = {
    "Content-Type": "application/json",
    "Accept": "application/json",
}
payload = {
    "name": "Ann O'Neil",
    "age": 34,
    "admin": False,
    "tags": ["a", "b"],
    "address": {
        "city": "Zürich",
        "zip": None,
    },
    "score": 9.5,
    "meta": {},
}

response = requests.post(url, headers=headers, json=payload)

print(response.status_code)
print(response.text)