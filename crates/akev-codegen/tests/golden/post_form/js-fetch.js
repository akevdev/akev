// Note: browsers don't let fetch() set the Cookie and Origin headers; they're silently dropped.
// The browser sends its own cookies instead (add credentials: "include" for cross-origin requests).
const response = await fetch("https://example.com/login", {
  method: "POST",
  headers: {
    "Content-Type": "application/x-www-form-urlencoded",
    "Cookie": "session=abc123; theme=dark",
    "Origin": "https://example.com"
  },
  body: new URLSearchParams({
    user: "ann@example.com",
    password: "p@ss w0rd!",
    remember: "on"
  })
});

console.log(response.status);
console.log(await response.text());