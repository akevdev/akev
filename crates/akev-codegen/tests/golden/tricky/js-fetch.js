// Note: fetch() sends header values as Latin-1, so non-ASCII values are passed as their UTF-8 bytes.
const response = await fetch("https://example.com/api/notes/1?q=a%20b&x=$y&z='\"", {
  method: "PATCH",
  headers: {
    "Content-Type": "text/plain; charset=utf-8",
    "X-Quote": 'it\'s "quoted" `cmd` $HOME %PATH% \\back\\',
    "X-Unicode": String.fromCharCode(...new TextEncoder().encode("héllo 日本 ✓")),
    "X-Eq": "=starts-with-equals"
  },
  body: `Line 1: it's "quoted" and 'single'
Line 2: $HOME \${HOME} \`whoami\` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\\slash \\n \\\\ """ ''' \`\`\` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
'@ at line start, and a trailing backslash \\`
});

console.log(response.status);
console.log(await response.text());