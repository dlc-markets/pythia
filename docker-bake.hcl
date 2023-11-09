target "docker-metadata-action" {}

target "pythia" {
  inherits = ["docker-metadata-action"]
  context = "./"
  dockerfile = "Dockerfile"

  platforms = [
    "linux/amd64",
    "linux/arm64",
  ]
}
