$body = @'
{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\nbreak \"q\" $x #{y} \u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}
'@

$response = Invoke-WebRequest -Uri 'https://example.com/graphql' `
    -Method Post `
    -ContentType 'application/json; charset=utf-8' `
    -Body $body

$response.StatusCode
$response.Content