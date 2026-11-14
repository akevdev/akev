wget --quiet --output-document=- \
  --method=PATCH \
  --header='Content-Type: text/plain; charset=utf-8' \
  --header='X-Quote: it'\''s "quoted" `cmd` $HOME %PATH% \back\' \
  --header='X-Unicode: héllo 日本 ✓' \
  --header='X-Eq: =starts-with-equals' \
  --body-data=$'Line 1: it\'s "quoted" and \'single\'\nLine 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y \x21\x21\n\tTabbed: back\\slash \\n \\\\ """ \'\'\' ``` --%\nUnicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n\'@ at line start, and a trailing backslash \\' \
  'https://example.com/api/notes/1?q=a%20b&x=$y&z='\''"'