import axios from "axios";

const response = await axios.request({
  method: "purge",
  url: "http://cache.local:8080/a/../b/[1]?x={y}",
  headers: {
    "Host": "example.com",
    "X-Dup": "one, two",
    "X-Empty": ""
  },
  data: "plain body without a content type"
});

console.log(response.status);
console.log(response.data);