from flask import Flask


def create_app():
    app = Flask(__name__)

    from . import auth
    from . import db

    db.init_app(app)
    app.register_blueprint(auth.bp)

    @app.errorhandler(404)
    def not_found(error):
        return "not found", 404

    return app
