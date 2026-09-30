package shop

import scala.sys.process._

object Exporter {
  def run(): Int = Seq("invctl", "export").!
}
