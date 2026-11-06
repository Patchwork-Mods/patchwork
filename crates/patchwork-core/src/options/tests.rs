use super::*;

fn field(kind: FieldKind) -> Field {
    Field {
        id: "f".to_owned(),
        title: "F".to_owned(),
        description: None,
        kind,
        value: Value::Bool(false),
        default: None,
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

#[test]
fn values_are_checked_against_their_fields() {
    let toggle = field(FieldKind::Toggle);
    assert_eq!(toggle.check(&Value::Bool(true)), Ok(Value::Bool(true)));
    assert!(toggle.check(&Value::Int(1)).is_err());

    let int = field(FieldKind::Int {
        min: Some(0),
        max: Some(10),
        step: None,
    });
    assert_eq!(int.check(&Value::Int(10)), Ok(Value::Int(10)));
    assert!(
        int.check(&Value::Int(-1))
            .unwrap_err()
            .contains("less than 0")
    );
    assert!(int.check(&Value::Float(1.5)).is_err());

    // whole numbers are taken for numbers
    let float = field(FieldKind::Float {
        min: None,
        max: Some(1.0),
        step: None,
    });
    assert_eq!(float.check(&Value::Int(1)), Ok(Value::Float(1.0)));
    assert!(float.check(&Value::Float(1.5)).is_err());
    assert!(float.check(&Value::Float(f64::NAN)).is_err());

    let line = field(FieldKind::Text { multiline: false });
    assert_eq!(line.check(&text("hi")), Ok(text("hi")));
    assert!(line.check(&text("a\nb")).is_err());
    let lines = field(FieldKind::Text { multiline: true });
    assert_eq!(lines.check(&text("a\nb")), Ok(text("a\nb")));

    let choice = field(FieldKind::Choice {
        choices: vec![Choice {
            id: "a".to_owned(),
            title: "A".to_owned(),
        }],
    });
    assert_eq!(choice.check(&text("a")), Ok(text("a")));
    assert!(choice.check(&text("A")).unwrap_err().contains("one of a"));

    let color = field(FieldKind::Color { alpha: false });
    assert_eq!(color.check(&text("#FF8800")), Ok(text("#ff8800")));
    assert!(color.check(&text("#ff880080")).is_err());
    assert!(color.check(&text("ff8800")).is_err());
    assert!(color.check(&text("#gg8800")).is_err());
    let alpha = field(FieldKind::Color { alpha: true });
    assert_eq!(alpha.check(&text("#ff880080")), Ok(text("#ff880080")));
}

#[test]
fn answers_go_to_groups_in_order() {
    let fields = |max| GroupKind::Fields {
        fields: vec![Field {
            kind: FieldKind::Int {
                min: None,
                max: Some(max),
                step: None,
            },
            ..field(FieldKind::Toggle)
        }],
    };
    let group = |max| Group {
        id: "g".to_owned(),
        title: "G".to_owned(),
        kind: fields(max),
        defaulted: false,
    };
    let form = Form {
        pages: vec![Page {
            title: String::new(),
            groups: vec![group(1), group(5)],
        }],
        ..Form::default()
    };
    let set = |v| Answer::values("g", [("f".to_owned(), Value::Int(v))].into());
    assert!(checked(&form, &[set(1), set(5)]).is_ok());
    assert!(checked(&form, &[set(5), set(1)]).is_err());
    // a third has no group now, so it's kept as it is
    assert!(checked(&form, &[set(1), set(5), set(99)]).is_ok());

    let unknown = Answer::values("g", [("nope".to_owned(), Value::Int(1))].into());
    assert!(checked(&form, &[unknown]).is_err());
}

#[test]
fn frontends_get_plain_json() {
    let group = Group {
        id: "g".to_owned(),
        title: "G".to_owned(),
        kind: GroupKind::Fields {
            fields: vec![field(FieldKind::Int {
                min: Some(0),
                max: None,
                step: None,
            })],
        },
        defaulted: true,
    };
    let json = serde_json::to_value(&group).unwrap();
    assert_eq!(json["kind"], "fields");
    assert_eq!(json["fields"][0]["type"], "int");
    assert_eq!(json["fields"][0]["min"], 0);
    assert_eq!(json["fields"][0]["value"], false);

    let answer: Answer =
        serde_json::from_str(r#"{"group":"g","values":{"a":1,"b":1.5,"c":"x","d":true}}"#).unwrap();
    assert_eq!(answer.values["a"], Value::Int(1));
    assert_eq!(answer.values["b"], Value::Float(1.5));
    assert_eq!(answer.values["c"], text("x"));
    assert_eq!(answer.values["d"], Value::Bool(true));
    // and answers that only pick look as they did
    let picks = serde_json::to_string(&Answer::picks("g", vec!["a".to_owned()])).unwrap();
    assert_eq!(picks, r#"{"group":"g","items":["a"]}"#);
}
