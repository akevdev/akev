<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'http://cache.local:8080/a/../b/[1]?x={y}',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'purge',
    CURLOPT_HTTPHEADER => [
        'Host: example.com',
        'X-Dup: one',
        'x-dup: two',
        'X-Empty;',
    ],
    CURLOPT_POSTFIELDS => 'plain body without a content type',
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}