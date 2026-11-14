import requests

url = "https://example.com/graphql"
headers = {
    "Content-Type": "application/json; charset=utf-8",
}
payload = {
    "__proto__": {
        "polluted": True,
    },
    "0": "zero",
    "big": 9007199254740993,
    "text": 'line\nbreak "q" $x #{y} é',
    "empty": [],
    "nested": [
        [1, 2],
        {
            "a": None,
        },
    ],
    "neg": -1.5e-7,
}

response = requests.post(url, headers=headers, json=payload)

print(response.status_code)
print(response.text)