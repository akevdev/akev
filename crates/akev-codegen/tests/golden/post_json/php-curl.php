<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://api.example.com/v1/users',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'POST',
    CURLOPT_HTTPHEADER => [
        'Content-Type: application/json',
        'Accept: application/json',
    ],
    CURLOPT_POSTFIELDS => json_encode([
        'name' => 'Ann O\'Neil',
        'age' => 34,
        'admin' => false,
        'tags' => ['a', 'b'],
        'address' => [
            'city' => 'Zürich',
            'zip' => null,
        ],
        'score' => 9.5,
        'meta' => new \stdClass(),
    ]),
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}