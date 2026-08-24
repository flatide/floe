from .cli import main


# Multiprocessing spawn re-imports this module; never re-run the CLI there.
if __name__ == "__main__":
    main()
