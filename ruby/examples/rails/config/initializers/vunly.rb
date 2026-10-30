# frozen_string_literal: true

# Configure Vunly once. The SDK reads neither the environment nor a file by itself: this initializer does, explicitly.
# VUNLY_API_KEY is a sandbox key (vk_test_...) and never goes into source, logs or params.
Vunly.configure do |config|
  config.api_key = ENV.fetch("VUNLY_API_KEY")
  config.base_url = ENV.fetch("VUNLY_API_URL")
  config.timeout = 10 # seconds for one attempt as a whole
  config.max_retries = 2
end
