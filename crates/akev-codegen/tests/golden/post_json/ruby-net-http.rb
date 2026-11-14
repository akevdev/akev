require 'json'
require 'net/http'

uri = URI('https://api.example.com/v1/users')

request = Net::HTTP::Post.new(uri)
request['Content-Type'] = 'application/json'
request['Accept'] = 'application/json'
request.body = JSON.generate({
  'name' => "Ann O'Neil",
  'age' => 34,
  'admin' => false,
  'tags' => ['a', 'b'],
  'address' => {
    'city' => 'Zürich',
    'zip' => nil
  },
  'score' => 9.5,
  'meta' => {}
})

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body