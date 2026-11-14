(
echo LS1YeVoNCkNvbnRlbnQtRGlzcG9zaXRpb246IGZvcm0tZGF0YTsgbmFtZT0ibm90ZSINCg0KaGkg
echo G1sxbSJ0aGVyZSINCi0tWHlaLS0NCg==
) > akev-body.b64
certutil -f -decode akev-body.b64 akev-body.bin >nul
curl "https://example.com/upload" ^
  -H "Content-Type: multipart/form-data; boundary=XyZ" ^
  --data-binary "@akev-body.bin"