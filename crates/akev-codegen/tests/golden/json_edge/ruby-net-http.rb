require 'json'
require 'net/http'

uri = URI('https://example.com/graphql')

request = Net::HTTP::Post.new(uri)
request['Content-Type'] = 'application/json; charset=utf-8'
request.body = JSON.generate({
  '__proto__' => {
    'polluted' => true
  },
  '0' => 'zero',
  'big' => 9007199254740993,
  'text' => "line\nbreak \"q\" $x \#{y} é",
  'empty' => [],
  'nested' => [
    [1, 2],
    {
      'a' => nil
    }
  ],
  'neg' => -1.5e-7
})

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body