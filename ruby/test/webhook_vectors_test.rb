# frozen_string_literal: true

require_relative "test_helper"

# The shared webhook vectors (webhook-vectors.json, an identical copy in every SDK; a repository check keeps the copies
# identical). The signatures were computed outside any SDK. "expect" is "ok", a WebhookError reason, or "config" (an
# ArgumentError raised before the signature is looked at).
class WebhookVectorsTest < Minitest::Test
  FILE = JSON.parse(File.read(File.join(__dir__, "webhook-vectors.json")))
  NOW = Time.at(FILE["now"])

  def test_there_are_enough_vectors
    assert_operator FILE["vectors"].size, :>=, 20
  end

  FILE["vectors"].each_with_index do |v, i|
    slug = v["name"].gsub(/\W+/, "_")
    define_method("test_vector_#{i}_#{slug}") do
      got =
        begin
          Vunly.verify_webhook(v["payload"], v["header"], v["secrets"], now: NOW)
          "ok"
        rescue Vunly::WebhookError => e
          e.reason
        rescue ArgumentError
          "config"
        end
      assert_equal v["expect"], got, v["name"]
    end
  end
end
