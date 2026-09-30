package shop

import scopt.OParser

case class Config(command: String = "")

object Main {
  val builder = OParser.builder[Config]
  val parser = {
    import builder._
    OParser.sequence(programName("invctl"), cmd("export").action((_, c) => c.copy(command = "export")))
  }
}
