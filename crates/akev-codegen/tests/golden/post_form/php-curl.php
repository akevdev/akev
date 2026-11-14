<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://example.com/login',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'POST',
    CURLOPT_HTTPHEADER => [
        'Content-Type: application/x-www-form-urlencoded',
        'Cookie: session=abc123; theme=dark',
        'Origin: https://example.com',
    ],
    CURLOPT_POSTFIELDS => http_build_query([
        'user' => 'ann@example.com',
        'password' => 'p@ss w0rd!',
        'remember' => 'on',
    ]),
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}