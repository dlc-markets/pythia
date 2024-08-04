variable NAME {
  default = "pythia"
}

target "docker-metadata-action" {}

target "default" {
  context = "./packages/${NAME}"
}

target "ci" {
  inherits = ["docker-metadata-action", "default"]
}
