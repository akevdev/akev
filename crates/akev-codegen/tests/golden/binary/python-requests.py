import base64

import requests

url = "https://upload.example.com/files/logo.png"
headers = {
    "Content-Type": "image/png",
    "X-Upload-Token": "t0k3n",
}
payload = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=")

response = requests.put(url, headers=headers, data=payload)

print(response.status_code)
print(response.text)