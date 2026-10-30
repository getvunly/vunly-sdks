# frozen_string_literal: true

Rails.application.routes.draw do
  root to: ->(_env) { [200, { "content-type" => "text/plain" }, ["vunly rails example\n"]] }
  post "/generate", to: "generate#create"
  post "/summarize", to: "generate#summarize"
end
