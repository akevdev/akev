<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://api.example.com/v1/items?page=2&sort=-created',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_ENCODING => '',
    CURLOPT_HTTPHEADER => [
        'Accept: application/json',
        'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc',
        'User-Agent: Akev/1.0',
        'X-Request-Id: 42',
    ],
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}