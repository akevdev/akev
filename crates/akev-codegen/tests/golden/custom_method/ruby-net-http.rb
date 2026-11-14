require 'net/http'

uri = URI('http://cache.local:8080/a/../b/[1]?x={y}')

request = Net::HTTPGenericRequest.new('purge', true, true, uri)
request['Host'] = 'example.com'
request['X-Dup'] = 'one'
request.add_field('x-dup', 'two')
request['X-Empty'] = ''
request.body = 'plain body without a content type'

response = Net::HTTP.start(uri.hostname, uri.port, use_ssl: uri.scheme == 'https') do |http|
  http.request(request)
end

puts response.code
puts response.body