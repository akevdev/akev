const response = await fetch("https://example.com/upload", {
  method: "POST",
  headers: {
    "Content-Type": "multipart/form-data; boundary=XyZ"
  },
  body: `--XyZ\r
Content-Disposition: form-data; name="note"\r
\r
hi \x1B[1m"there"\r
--XyZ--\r
`
});

console.log(response.status);
console.log(await response.text());