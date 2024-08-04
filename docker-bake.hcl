variable NAME {
  default = "pythia"
}

target "docker-metadata-action" {}

target "default" {
  context = "."
  dockerfile = "./packages/${NAME}/Dockerfile"
}

target "ci" {
  inherits = ["docker-metadata-action", "default"]
}
