curl 'https://example.com/graphql' \
  -H 'Content-Type: application/json; charset=utf-8' \
  --data-raw '{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\nbreak \"q\" $x #{y} \u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}'