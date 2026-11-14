import httpx

url = "https://example.com/api/notes/1?q=a%20b&x=$y&z='\""
headers = {
    "Content-Type": "text/plain; charset=utf-8",
    "X-Quote": 'it\'s "quoted" `cmd` $HOME %PATH% \\back\\',
    "X-Unicode": "héllo 日本 ✓".encode(),
    "X-Eq": "=starts-with-equals",
}
payload = """Line 1: it's "quoted" and 'single'
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
\tTabbed: back\\slash \\n \\\\ "\"\" ''' ``` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
'@ at line start, and a trailing backslash \\"""

response = httpx.patch(url, headers=headers, content=payload)

print(response.status_code)
print(response.text)