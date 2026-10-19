// Lists the exports of a TypeScript module and the test titles of a test file with the TypeScript compiler API.
// Plain JS so both parity/tap.ts (inside tsx) and scripts/verify-unit (plain node) can use it.
import fs from 'node:fs';
import ts from 'typescript';

const modifiersOf = node => (ts.canHaveModifiers(node) ? ts.getModifiers(node) ?? [] : []);
const hasModifier = (node, kind) => modifiersOf(node).some(modifier => modifier.kind === kind);
const parse = file => ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, file.endsWith('x') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);

/**
 * Exports of `file`: [{name, kind}], kind one of function | const | class | enum | type | default | default-function |
 * star. `type` = interface / type alias / type-only re-export (no runtime binding). `star` = `export * from 'x'`
 * (name = 'x').
 */
export function listExports(file) {
	const source = parse(file);
	const out = [];
	const add = (name, kind) => { if (!out.some(entry => entry.name === name)) out.push({name, kind}); };
	for (const statement of source.statements) {
		if (ts.isExportDeclaration(statement)) {
			if (!statement.exportClause) { add(statement.moduleSpecifier?.text ?? '*', 'star'); continue; }
			if (ts.isNamespaceExport(statement.exportClause)) { add(statement.exportClause.name.text, statement.isTypeOnly ? 'type' : 'const'); continue; }
			for (const element of statement.exportClause.elements) add(element.name.text, statement.isTypeOnly || element.isTypeOnly ? 'type' : 'const');
			continue;
		}
		if (ts.isExportAssignment(statement)) { add('default', 'default'); continue; }
		if (!hasModifier(statement, ts.SyntaxKind.ExportKeyword)) continue;
		if (hasModifier(statement, ts.SyntaxKind.DefaultKeyword)) { add('default', ts.isFunctionDeclaration(statement) ? 'default-function' : 'default'); continue; }
		if (ts.isFunctionDeclaration(statement) && statement.name) add(statement.name.text, 'function');
		else if (ts.isClassDeclaration(statement) && statement.name) add(statement.name.text, 'class');
		else if (ts.isEnumDeclaration(statement)) add(statement.name.text, 'enum');
		else if (ts.isInterfaceDeclaration(statement) || ts.isTypeAliasDeclaration(statement) || ts.isModuleDeclaration(statement)) add(statement.name.text, 'type');
		else if (ts.isVariableStatement(statement)) {
			const visit = name => {
				if (ts.isIdentifier(name)) add(name.text, 'const');
				else for (const element of name.elements) if (!ts.isOmittedExpression(element)) visit(element.name);
			};
			for (const declaration of statement.declarationList.declarations) visit(declaration.name);
		}
	}
	return out;
}

/**
 * `test('title', ...)` / `it('title', ...)` calls (also .only/.skip/.todo) in a test file: [{title, line, dynamic}].
 * A title that is not a plain literal yields its source text and dynamic: true.
 */
export function listTestTitles(file) {
	const source = parse(file);
	const out = [];
	const isTestCallee = callee => (ts.isIdentifier(callee) && (callee.text === 'test' || callee.text === 'it'))
		|| (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression) && ['test', 'it'].includes(callee.expression.text) && ['only', 'skip', 'todo'].includes(callee.name.text));
	const visit = node => {
		if (ts.isCallExpression(node) && isTestCallee(node.expression) && node.arguments.length) {
			const first = node.arguments[0], line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
			if (ts.isStringLiteral(first) || ts.isNoSubstitutionTemplateLiteral(first)) out.push({title: first.text, line, dynamic: false});
			else out.push({title: first.getText(source), line, dynamic: true});
		}
		ts.forEachChild(node, visit);
	};
	visit(source);
	return out;
}
