import httpx

url = "http://cache.local:8080/a/../b/[1]?x={y}"
headers = {
    "Host": "example.com",
    "X-Dup": "one, two",
    "X-Empty": "",
}
payload = "plain body without a content type"

# Note: httpx upper-cases the method, so this sends PURGE.
response = httpx.request("purge", url, headers=headers, content=payload)

print(response.status_code)
print(response.text)