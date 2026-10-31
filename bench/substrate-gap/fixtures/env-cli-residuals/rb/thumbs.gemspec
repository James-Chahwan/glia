Gem::Specification.new do |spec|
  spec.name = "thumbs"
  spec.version = "0.1.0"
  spec.executables = ["thumbs"]
  spec.executables.concat(%w[thumbs-gc])
  spec.executables += ["thumbs-ls"]
end
