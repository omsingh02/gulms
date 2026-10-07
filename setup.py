from setuptools import setup, find_packages

setup(
    name="gulms",
    version="1.0.0",
    description="Python client and CLI for Galgotias University LMS (Moodle REST API)",
    packages=find_packages(),
    install_requires=[
        "requests>=2.28.0",
        "python-pptx>=0.6.21",
        "Pillow>=9.0.0",
    ],
    extras_require={
        "ocr": ["pytesseract>=0.3.10"],
        "pdf": ["pymupdf>=1.23.0"],
        "all": ["pytesseract>=0.3.10", "pymupdf>=1.23.0"],
    },
    entry_points={
        "console_scripts": [
            "gulms=gulms.cli:main",
        ],
    },
    python_requires=">=3.8",
)
