import base64

import httpx

url = "https://upload.example.com/files/logo.png"
headers = {
    "Content-Type": "image/png",
    "X-Upload-Token": "t0k3n",
}
payload = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=")

response = httpx.put(url, headers=headers, content=payload)

print(response.status_code)
print(response.text)