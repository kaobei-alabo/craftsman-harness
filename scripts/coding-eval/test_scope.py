"""Retain original checks while allowing formatting and extra test coverage."""
import ast
import re


def retained_checks(language, original, changed):
    if original in changed:
        return True
    if language == 'python':
        try:
            before = [ast.dump(node.test) for node in ast.walk(ast.parse(original)) if isinstance(node, ast.Assert)]
            after = [ast.dump(node.test) for node in ast.walk(ast.parse(changed)) if isinstance(node, ast.Assert)]
        except SyntaxError:
            return False
        return bool(before) and all(test in after for test in before)
    # Formatting preserves lexical tokens, including the content of literals.
    pattern = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|//[^\n]*|/\*[\s\S]*?\*/|[A-Za-z_]\w*|\d+|[^\s]')
    def tokens(text):
        result = [match[0] for match in pattern.finditer(text) if not match[0].startswith(('//','/*'))]
        return [token for i,token in enumerate(result) if not (token==',' and i+1<len(result) and result[i+1] in {')',']','}'})]
    before,after=tokens(original),tokens(changed)
    # Additional tests can add tokens without removing the original check.
    index=0
    for token in after:
        if index<len(before) and token==before[index]:index+=1
    return index==len(before)


if __name__=='__main__':
    import unittest
    class Cases(unittest.TestCase):
        def test_rust_formatting_and_extra_tests(self):
            before='#[test] fn ascii(){assert_eq!(f("abc",2),"ab");}'
            self.assertTrue(retained_checks('rust',before,'#[test]\nfn ascii() {\n assert_eq!(f("abc", 2), "ab");\n}\n#[test]fn extra(){}'))
        def test_changed_expectation_is_not_formatting(self):
            self.assertFalse(retained_checks('rust','assert_eq!(f("abc",2),"ab");','assert_eq!(f("abc",2),"abc");'))
        def test_string_whitespace_is_preserved(self):
            self.assertFalse(retained_checks('javascript','assert.equal(f(),"a b");','assert.equal(f(),"ab");'))
        def test_python_assertions_survive_formatting(self):
            self.assertTrue(retained_checks('python','assert f(1)==2','assert f(1) == 2\nassert f(2)==3'))
            self.assertFalse(retained_checks('python','assert f(1)==2','assert True'))
    unittest.main()
