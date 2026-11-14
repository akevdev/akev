import axios from "axios";

const response = await axios.request({
  method: "post",
  url: "https://example.com/login",
  headers: {
    "Content-Type": "application/x-www-form-urlencoded",
    "Cookie": "session=abc123; theme=dark",
    "Origin": "https://example.com"
  },
  data: new URLSearchParams({
    user: "ann@example.com",
    password: "p@ss w0rd!",
    remember: "on"
  })
});

console.log(response.status);
console.log(response.data);