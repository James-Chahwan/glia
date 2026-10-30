import pathlib


def scaffold(root):
    pathlib.Path(root, "V1__init.sql").write_text("CREATE TABLE manifests (id INT);\n")
