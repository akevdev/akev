# Note: Invoke-WebRequest refuses to send non-ASCII header values ("Request headers must contain only ASCII characters"); the X-Unicode header value needs to be ASCII for this to work as captured.
$headers = @{
    'X-Quote' = 'it''s "quoted" `cmd` $HOME %PATH% \back\'
    'X-Unicode' = 'héllo 日本 ✓'
    'X-Eq' = '=starts-with-equals'
}
$body = 'Line 1: it''s "quoted" and ''single''
Line 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\slash \n \\ """ '''''' ``` --%
Unicode: héllo — 日本語 🚀 ‘‘smart’’ “quotes”
''@ at line start, and a trailing backslash \'

$response = Invoke-WebRequest -Uri 'https://example.com/api/notes/1?q=a%20b&x=$y&z=''"' `
    -Method Patch `
    -Headers $headers `
    -ContentType 'text/plain; charset=utf-8' `
    -Body ([System.Text.Encoding]::UTF8.GetBytes($body))

$response.StatusCode
$response.Content