# Python language-server notes

Verified with basedpyright 1.40.2: after `initialized`, it requests configuration for both `python` and `basedpyright`. Reply to each request with one value per item: `{"pythonPath":"<interpreter>"}` for the `python` section and `{}` for other sections. It honors `python.pythonPath` from `workspace/configuration` for third-party imports, including an explicitly configured `.venv-hidden` interpreter.
