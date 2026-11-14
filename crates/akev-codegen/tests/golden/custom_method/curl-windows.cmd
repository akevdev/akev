curl "http://cache.local:8080/a/../b/[1]?x={y}" ^
  -X purge ^
  -H "Host: example.com" ^
  -H "X-Dup: one" ^
  -H "x-dup: two" ^
  -H "X-Empty;" ^
  -H "Content-Type:" ^
  --data-raw "plain body without a content type" ^
  --globoff ^
  --path-as-is