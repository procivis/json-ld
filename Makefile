.PHONY: readme json-ld/README.md clean

readme: syntax/README.md core/README.md context-processing/README.md expansion/README.md compaction/README.md testing/README.md tests/README.md README.md

syntax/README.md:
	make -C syntax readme

core/README.md:
	make -C core readme

context-processing/README.md:
	make -C context-processing readme

expansion/README.md:
	make -C expansion readme

compaction/README.md:
	make -C compaction readme

json-ld/README.md:
	make -C json-ld readme

testing/README.md:
	make -C testing readme

tests/README.md:
	make -C tests readme

README.md: json-ld/README.md
	cp json-ld/README.md .

clean:
	make -C syntax clean
	make -C core clean
	make -C context-processing clean
	make -C expansion clean
	make -C compaction clean
	make -C json-ld clean
	make -C testing clean
	make -C tests clean
	rm README