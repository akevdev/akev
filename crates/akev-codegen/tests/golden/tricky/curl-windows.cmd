curl ^"https://example.com/api/notes/1?q=a^%^20b^&x=^$y^&z='^\^"^" ^
  -X PATCH ^
  -H "Content-Type: text/plain; charset=utf-8" ^
  -H ^"X-Quote: it's ^\^"quoted^\^" `cmd` ^$HOME ^%^PATH^%^ ^\back^\^\^" ^
  -H "X-Unicode: héllo 日本 ✓" ^
  -H "X-Eq: =starts-with-equals" ^
  --data-raw ^"Line 1: it's ^\^"quoted^\^" and 'single'^

Line 2: ^$HOME ^$^{HOME^} `whoami` ^$(id) ^%^PATH^%^ 100^%^ ^#^{x^} ^#^@y ^!^!^

	Tabbed: back^\slash ^\n ^\^\ ^\^"^\^"^\^" ''' ``` --^%^

Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”^

'^@ at line start, and a trailing backslash ^\^\^"