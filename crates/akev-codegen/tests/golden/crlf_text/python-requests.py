import requests

url = "https://example.com/upload"
headers = {
    "Content-Type": "multipart/form-data; boundary=XyZ",
}
payload = """--XyZ\r
Content-Disposition: form-data; name="note"\r
\r
hi \x1B[1m"there"\r
--XyZ--\r
"""

response = requests.post(url, headers=headers, data=payload)

print(response.status_code)
print(response.text)