// Note: fetch() always derives Host from the URL, so "Host: example.com" is not sent.
const response = await fetch("http://cache.local:8080/a/../b/[1]?x={y}", {
  method: "purge",
  headers: {
    "Host": "example.com",
    "X-Dup": "one, two",
    "X-Empty": ""
  },
  body: "plain body without a content type"
});

console.log(response.status);
console.log(await response.text());