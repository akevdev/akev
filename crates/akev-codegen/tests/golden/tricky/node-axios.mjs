import axios from "axios";

// Note: Node.js sends header values as Latin-1, so non-ASCII values are passed as their UTF-8 bytes.
const response = await axios.request({
  method: "patch",
  url: "https://example.com/api/notes/1?q=a%20b&x=$y&z='\"",
  headers: {
    "Content-Type": "text/plain; charset=utf-8",
    "X-Quote": 'it\'s "quoted" `cmd` $HOME %PATH% \\back\\',
    "X-Unicode": Buffer.from("héllo 日本 ✓").toString("latin1"),
    "X-Eq": "=starts-with-equals"
  },
  data: `Line 1: it's "quoted" and 'single'
Line 2: $HOME \${HOME} \`whoami\` $(id) %PATH% 100% #{x} #@y !!
	Tabbed: back\\slash \\n \\\\ """ ''' \`\`\` --%
Unicode: héllo — 日本語 🚀 ‘smart’ “quotes”
'@ at line start, and a trailing backslash \\`
});

console.log(response.status);
console.log(response.data);