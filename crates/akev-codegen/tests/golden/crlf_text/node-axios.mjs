import axios from "axios";

const response = await axios.request({
  method: "post",
  url: "https://example.com/upload",
  headers: {
    "Content-Type": "multipart/form-data; boundary=XyZ"
  },
  data: `--XyZ\r
Content-Disposition: form-data; name="note"\r
\r
hi \x1B[1m"there"\r
--XyZ--\r
`
});

console.log(response.status);
console.log(response.data);