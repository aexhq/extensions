from pathlib import Path
from packaging.version import Version

assert str(Version("1.2.3")) == "1.2.3"
Path(__file__).with_name("prepared").write_text("ready")
