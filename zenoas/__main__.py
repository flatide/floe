from .cli import main

# guard required: the render process is spawned and re-imports the main
# module; without this the child would re-run the CLI
if __name__ == "__main__":
    main()
