printf '%s' $'Line 1: it\'s "quoted" and \'single\'\nLine 2: $HOME ${HOME} `whoami` $(id) %PATH% 100% #{x} #@y \x21\x21\n\tTabbed: back\\slash \\n \\\\ """ \'\'\' ``` --%\nUnicode: héllo — 日本語 🚀 ‘smart’ “quotes”\n\'@ at line start, and a trailing backslash \\' | http PATCH 'https://example.com/api/notes/1?q=a%20b&x=$y&z='\''"' \
  'Content-Type:text/plain; charset=utf-8' \
  'X-Quote:it'\''s "quoted" `cmd` $HOME %PATH% \back\' \
  'X-Unicode:héllo 日本 ✓' \
  'X-Eq:\=starts-with-equals'