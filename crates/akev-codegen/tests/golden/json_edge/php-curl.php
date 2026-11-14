<?php

$curl = curl_init();

curl_setopt_array($curl, [
    CURLOPT_URL => 'https://example.com/graphql',
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_CUSTOMREQUEST => 'POST',
    CURLOPT_HTTPHEADER => [
        'Content-Type: application/json; charset=utf-8',
    ],
    CURLOPT_POSTFIELDS => '{
  "__proto__": {"polluted": true},
  "0": "zero",
  "big": 9007199254740993,
  "text": "line\\nbreak \\"q\\" $x #{y} \\u00e9",
  "empty": [],
  "nested": [[1, 2], {"a": null}],
  "neg": -1.5e-7
}',
]);

$response = curl_exec($curl);

if ($response === false) {
    echo 'cURL error: ' . curl_error($curl) . PHP_EOL;
} else {
    echo curl_getinfo($curl, CURLINFO_RESPONSE_CODE) . PHP_EOL;
    echo $response . PHP_EOL;
}