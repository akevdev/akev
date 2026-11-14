(
echo iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w=
) > akev-body.b64
certutil -f -decode akev-body.b64 akev-body.bin >nul
curl "https://upload.example.com/files/logo.png" ^
  -X PUT ^
  -H "Content-Type: image/png" ^
  -H "X-Upload-Token: t0k3n" ^
  --data-binary "@akev-body.bin"