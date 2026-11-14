require 'net/http'

uri = URI('https://example.com/login')

request = Net::HTTP::Post.new(uri)
request['Content-Type'] = 'application/x-www-form-urlencoded'
request['Cookie'] = 'session=abc123; theme=dark'
request['Origin'] = 'https://example.com'
request.body = URI.encode_www_form([
  ['user', 'ann@example.com'],
  ['password', 'p@ss w0rd!'],
  ['remember', 'on']
])

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body