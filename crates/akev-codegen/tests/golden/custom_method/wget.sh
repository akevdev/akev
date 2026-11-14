# Note: Wget2 doesn't send headers with empty values (X-Empty).
wget --quiet --output-document=- \
  --method=purge \
  --header='Host: example.com' \
  --header='X-Dup: one, two' \
  --header='X-Empty: ' \
  --body-data='plain body without a content type' \
  'http://cache.local:8080/a/../b/[1]?x={y}'