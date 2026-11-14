printf '%s' 'plain body without a content type' | http purge 'http://cache.local:8080/a/../b/[1]?x={y}' \
  'Host:example.com' \
  'X-Dup:one' \
  'x-dup:two' \
  'X-Empty;' \
  'Content-Type:'