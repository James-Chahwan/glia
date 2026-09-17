from flask import Flask

app = Flask(__name__)


@app.route('/users/<int:uid>')
def get_user(uid):
    return {'id': uid}


@app.route('/files/<path:subpath>')
def get_file(subpath):
    return {'path': subpath}
