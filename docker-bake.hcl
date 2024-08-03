variable "ECR_REGISTRY" {
  default = ""
}

variable PROJECT {
  default = "dlc-markets"
}

variable NAME {
  default = "pythia"
}

variable ARCH {
  default = ""
}

target "docker-metadata-action" {}

target "default" {
  context = "./"
  dockerfile = "Dockerfile"
}

target "ci" {
  inherits = ["docker-metadata-action", "default"]
}
