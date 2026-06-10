variable "database_url" {
  type    = string
  default = "postgres://omniscient:omniscient@localhost:5432/omniscient?sslmode=disable"
}

env "local" {
  src = "file://schema.sql"
  url = var.database_url
  dev = "docker://postgres/16/dev?sslmode=disable"
}
