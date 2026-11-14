<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://upload.example.com/files/logo.png',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'PUT',
    CURLOPT_HTTPHEADER => [
        'Content-Type: image/png',
        'X-Upload-Token: t0k3n',
    ],
    CURLOPT_POSTFIELDS => base64_decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ//4iJ1w='),
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}